// `moza-rev configure` — detect installed racing games and offer to
// enable their UDP telemetry output. Read-only by default; any file
// edit prompts for `y/N` confirmation and writes a `.bak` backup first.

use std::env;
use std::fs;
use std::io::{self, BufRead, ErrorKind, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use serde_json::ser::{PrettyFormatter, Serializer};

use crate::{assetto_corsa, assetto_corsa_competizione, madness, outgauge};

/// ANSI styling for the configure output — bold/cyan game titles, green
/// ✓, red ✘, yellow ⚠. Auto-disabled when stdout isn't a terminal or
/// when `NO_COLOR` is set (https://no-color.org).
mod style {
    use std::io::IsTerminal;
    use std::sync::OnceLock;

    fn enabled() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
        })
    }

    pub fn title(s: &str) -> String {
        if enabled() {
            format!("\x1b[1;36m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn ok() -> &'static str {
        if enabled() {
            "\x1b[32m✓\x1b[0m"
        } else {
            "✓"
        }
    }
    pub fn cross() -> &'static str {
        if enabled() {
            "\x1b[31m✘\x1b[0m"
        } else {
            "✘"
        }
    }
    pub fn warn() -> &'static str {
        if enabled() {
            "\x1b[33m⚠\x1b[0m"
        } else {
            "⚠"
        }
    }
}

/// Steam app ids for games we know how to handle (or recognize).
const WF2_APP_ID: &str = "1203190";
const DR2_APP_ID: &str = "690790";
const DIRT_SHOWDOWN_APP_ID: &str = "201700";
const BEAMNG_APP_ID: &str = "284160";
const AMS2_APP_ID: &str = "1066890";
const AC_APP_ID: &str = "244210";
const AC_INSTALL_DIR: &str = "assettocorsa";
const ACC_APP_ID: &str = "805550";
const ACC_DEFAULT_PORT: i64 = assetto_corsa_competizione::DEFAULT_PORT as i64;

/// Detect-only — these run on EGO/proprietary engines without native UDP
/// telemetry; the only path is memory injection (SpaceMonkey on Windows).
const NO_TELEMETRY_GAMES: &[(&str, &str)] = &[("228380", "Wreckfest"), ("1038250", "DIRT 5")];

/// Games that have native UDP telemetry but use protocols moza-rev's
/// listener doesn't speak yet (Assetto Corsa family, Madness engine).
/// Detected and reported here so the status report is complete; users
/// can enable telemetry manually if they want to use other tools.
struct ManualEntry {
    app_id: &'static str,
    name: &'static str,
    notes: &'static str,
}

const MANUAL_TELEMETRY_GAMES: &[ManualEntry] = &[
    ManualEntry {
        app_id: "3917090",
        name: "Assetto Corsa Rally",
        notes: "  Built on UE5; Kunos exposes telemetry via a Windows shared-memory\n  \
                ring buffer (binary symbols `KunosPhysicalProperties`, \n  \
                `bSharedMemoryBufferCyclic`), not UDP. Same shape as iRacing on\n  \
                Linux — a Proton-side bridge would be needed to surface it as UDP.\n  \
                Reference: https://luizzak.itch.io/racing-overlay/devlog/1321475/assetto-corsa-rally-telemetry-support\n  \
                (uses an `ACRallyMemReader` companion).",
    },
    ManualEntry {
        app_id: "1551360",
        name: "Forza Horizon 5",
        notes: "  In game: Settings → HUD and Gameplay → Data Out: On,\n  \
                  Data Out IP Address: 127.0.0.1\n  \
                  Data Out IP Port: 9999  (must match `--forza-port`)\n  \
                Wire format is locked to Dash; no in-game format selector.\n  \
                Settings live inside Proton's compatdata in a format we\n  \
                haven't reverse-engineered, so this is in-game only.",
    },
    ManualEntry {
        app_id: "1293830",
        name: "Forza Horizon 4",
        notes: "  Same Data Out setup as FH5 (Settings → HUD and Gameplay).\n  \
                Same Dash UDP wire format; moza-rev's Forza listener\n  \
                handles both transparently.",
    },
];

/// Steam library paths to search for installed games.
fn steam_library_roots() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect()
}

/// Find a game's install directory under `steamapps/common/<subdir>`
/// across both Flatpak and native Steam install locations.
fn steam_install_path(install_subdir: &str) -> Option<PathBuf> {
    for root in steam_library_roots() {
        let p = root.join("steamapps/common").join(install_subdir);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Find the Proton "Documents" path for a Steam app id, across both
/// Flatpak and native Steam install locations.
fn proton_documents(app_id: &str) -> Option<PathBuf> {
    for root in steam_library_roots() {
        let docs = root
            .join("steamapps/compatdata")
            .join(app_id)
            .join("pfx/drive_c/users/steamuser/Documents");
        if docs.exists() {
            return Some(docs);
        }
    }
    None
}

/// Check whether a Steam app is installed by looking for its manifest file.
/// `steamapps/appmanifest_<id>.acf` is the authoritative source — directory
/// scans under `steamapps/common` get tripped up by case-sensitive names.
fn game_installed(app_id: &str) -> bool {
    steam_library_roots().into_iter().any(|root| {
        root.join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"))
            .exists()
    })
}

pub fn run() -> ExitCode {
    println!(
        "{}\n",
        style::title("moza-rev configure: scanning for installed racing games")
    );

    let mut any_handled = false;

    if game_installed(WF2_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_wf2() {
            eprintln!("Wreckfest 2: error: {e}\n");
        }
    }
    if game_installed(DR2_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_dr2_style("DiRT Rally 2.0", DR2_APP_ID, "DiRT Rally 2.0") {
            eprintln!("DiRT Rally 2.0: error: {e}\n");
        }
    }
    if game_installed(DIRT_SHOWDOWN_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_dr2_style("DiRT Showdown", DIRT_SHOWDOWN_APP_ID, "DiRT Showdown") {
            eprintln!("DiRT Showdown: error: {e}\n");
        }
    }
    if game_installed(AMS2_APP_ID) {
        any_handled = true;
        handle_ams2();
    }
    if game_installed(BEAMNG_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_beamng() {
            eprintln!("BeamNG.drive: error: {e}\n");
        }
    }
    if game_installed(AC_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_ac() {
            eprintln!("Assetto Corsa: error: {e}\n");
        }
    }
    if game_installed(ACC_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_acc_competizione() {
            eprintln!("Assetto Corsa Competizione: error: {e}\n");
        }
    }
    for entry in MANUAL_TELEMETRY_GAMES {
        if game_installed(entry.app_id) {
            any_handled = true;
            println!("{}", style::title(entry.name));
            println!("{}\n", entry.notes);
        }
    }
    for (app_id, name) in NO_TELEMETRY_GAMES {
        if game_installed(app_id) {
            any_handled = true;
            println!("{}", style::title(name));
            println!(
                "  {} detected, but the game has no native UDP telemetry.",
                style::cross()
            );
            println!(
                "  The only path on Linux is memory injection via SpaceMonkey under \
                 Wine — fragile and out of scope for moza-rev.\n"
            );
        }
    }

    if !any_handled {
        println!("No supported games detected under Steam's standard library paths.");
        println!(
            "Searched: {}",
            steam_library_roots()
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    ExitCode::SUCCESS
}

//
// Wreckfest 2 — JSON config
//

fn handle_wf2() -> io::Result<()> {
    println!("{}", style::title("Wreckfest 2"));
    let Some(docs) = proton_documents(WF2_APP_ID) else {
        println!(
            "  Installed but never launched — start the game once so it generates its config, then re-run.\n"
        );
        return Ok(());
    };
    let Some(config_path) = find_wf2_config(&docs) else {
        println!(
            "  Couldn't locate telemetry/config.json under {}.\n",
            docs.display()
        );
        return Ok(());
    };

    let raw = fs::read_to_string(&config_path)?;
    let mut doc: Value = serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let Some(udp_block) = doc
        .get_mut("udp")
        .and_then(|v| v.as_array_mut())
        .and_then(|a| a.get_mut(0))
    else {
        println!("  config.json doesn't have the expected `udp` array — leaving it alone.\n");
        return Ok(());
    };

    let current_enabled = udp_block
        .get("enabled")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let current_port = udp_block
        .get("port")
        .and_then(|v| v.as_str().or_else(|| v.as_str()))
        .map(str::to_owned);

    println!("  config: {}", config_path.display());
    println!(
        "  current: enabled={current_enabled}, port={}",
        current_port.as_deref().unwrap_or("?")
    );

    if current_enabled == 1 {
        println!(
            "  {} telemetry already enabled — no changes needed.\n",
            style::ok()
        );
        return Ok(());
    }

    println!(
        "  proposed: enabled=1 (port unchanged at {})",
        current_port.as_deref().unwrap_or("?")
    );
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }

    udp_block["enabled"] = Value::from(1);
    let new_raw = serde_json::to_string_pretty(&doc)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_with_backup(&config_path, &new_raw)?;
    println!("  {} written.\n", style::ok());
    Ok(())
}

fn find_wf2_config(docs: &Path) -> Option<PathBuf> {
    let base = docs.join("My Games").join("Wreckfest 2");
    if !base.exists() {
        return None;
    }
    // The userid (Steam ID) subdirectory varies per user; pick the first one.
    fs::read_dir(&base)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let candidate = entry.path().join("savegame/telemetry/config.json");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
}

//
// DR2 / DiRT Showdown — XML config (same format)
//

fn handle_dr2_style(name: &str, app_id: &str, my_games_subdir: &str) -> io::Result<()> {
    println!("{}", style::title(name));
    let Some(docs) = proton_documents(app_id) else {
        println!(
            "  Installed but never launched — start the game once so it generates its config, then re-run.\n"
        );
        return Ok(());
    };
    let config_path = docs
        .join("My Games")
        .join(my_games_subdir)
        .join("hardwaresettings/hardware_settings_config.xml");
    if !config_path.exists() {
        println!("  Couldn't find {}.\n", config_path.display());
        return Ok(());
    }

    let raw = fs::read_to_string(&config_path)?;
    let Some(element_range) = find_motion_or_udp_element(&raw) else {
        println!("  No <udp .../> or <motion .../> element found in the config. Leaving alone.\n");
        return Ok(());
    };
    let current_line = &raw[element_range.clone()];
    let current_enabled = parse_xml_attr(current_line, "enabled");
    let current_extradata = parse_xml_attr(current_line, "extradata");
    let current_ip = parse_xml_attr(current_line, "ip");
    let current_port = parse_xml_attr(current_line, "port");

    println!("  config: {}", config_path.display());
    println!(
        "  current: enabled={}, extradata={}, ip={}, port={}",
        current_enabled.as_deref().unwrap_or("?"),
        current_extradata.as_deref().unwrap_or("?"),
        current_ip.as_deref().unwrap_or("?"),
        current_port.as_deref().unwrap_or("?"),
    );

    let mut updates: Vec<(&str, &str)> = Vec::new();
    if current_enabled.as_deref() != Some("true") {
        updates.push(("enabled", "true"));
    }
    if current_extradata.as_deref() != Some("3") {
        updates.push(("extradata", "3"));
    }
    // If `ip` is set to a hardware-platform name (e.g. "dbox") rather than a
    // real IP, the game won't actually emit UDP. Fix it to localhost.
    let ip_is_real = current_ip
        .as_deref()
        .is_some_and(|ip| ip.parse::<IpAddr>().is_ok());
    if !ip_is_real {
        updates.push(("ip", "127.0.0.1"));
    }

    if updates.is_empty() {
        println!(
            "  {} telemetry already enabled with extradata=3 — no changes needed.\n",
            style::ok()
        );
        return Ok(());
    }

    let new_line = rewrite_xml_attrs(current_line, &updates);
    println!("  proposed:\n    -{current_line}\n    +{new_line}");
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }

    let mut new_raw = String::with_capacity(raw.len());
    new_raw.push_str(&raw[..element_range.start]);
    new_raw.push_str(&new_line);
    new_raw.push_str(&raw[element_range.end..]);
    write_with_backup(&config_path, &new_raw)?;
    println!("  {} written.\n", style::ok());
    Ok(())
}

/// Locate the UDP/motion element in a Codemasters EGO-engine config.
/// Newer games (DR1, DR2) wrap it as `<motion_platform><udp ... /></...>`;
/// older ones (DiRT Showdown, DiRT 2/3, F1 2010-2012) use `<motion ... />`
/// directly. Both have the same attribute set.
fn find_motion_or_udp_element(xml: &str) -> Option<std::ops::Range<usize>> {
    for needle in ["<udp ", "<motion "] {
        if let Some(start) = xml.find(needle)
            && let Some(rel_end) = xml[start..].find("/>")
        {
            return Some(start..start + rel_end + 2);
        }
    }
    None
}

/// Pull a single attribute out of a self-closing XML tag — naive but the
/// game's XML is consistent enough for it.
fn parse_xml_attr(element: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let i = element.find(&needle)?;
    let value_start = i + needle.len();
    let rel_end = element[value_start..].find('"')?;
    Some(element[value_start..value_start + rel_end].to_owned())
}

/// Replace specific attributes in an XML element, preserving the rest.
fn rewrite_xml_attrs(element: &str, updates: &[(&str, &str)]) -> String {
    let mut out = element.to_owned();
    for (key, value) in updates {
        let needle = format!("{key}=\"");
        if let Some(i) = out.find(&needle) {
            let value_start = i + needle.len();
            if let Some(rel_end) = out[value_start..].find('"') {
                let value_end = value_start + rel_end;
                out.replace_range(value_start..value_end, value);
            }
        }
    }
    out
}

//
// Automobilista 2 / Project CARS 2 — encrypted .sav, can't auto-edit
//
// We can still actively probe whether the host's broadcast-loopback
// quirk has been worked around: send a magic UDP packet to the limited
// broadcast address, see if our local listener receives it back.
//

fn handle_ams2() {
    let port = madness::DEFAULT_PORT;
    println!("{}", style::title("Automobilista 2"));
    println!("  Has UDP telemetry on port {port} using the Project CARS 2 format.");
    println!("  In game: Options → System → UDP Protocol Version: Project CARS 2,");
    println!("                              UDP Frequency: 1+");
    println!("  (Settings live in encrypted .sav files; can't auto-edit.)");

    match probe_broadcast_loopback(port) {
        ProbeResult::LoopbackWorks => {
            println!(
                "  {} broadcast loopback works on port {port} — game traffic will reach moza-rev.",
                style::ok()
            );
        }
        ProbeResult::NoLoopback => {
            println!(
                "  {} broadcast loopback NOT working on port {port} — game traffic won't reach moza-rev.",
                style::cross()
            );
            println!(
                "    Linux doesn't loop limited-broadcast (255.255.255.255) to local sockets."
            );
            println!("    Apply this iptables NAT redirect (one-time, reversible with -D):");
            println!(
                "      sudo iptables -t nat -I OUTPUT -p udp -d 255.255.255.255 --dport {port} \\"
            );
            println!("        -j DNAT --to-destination 127.0.0.1:{port}");
        }
        ProbeResult::CouldNotProbe(reason) => {
            println!(
                "  {} couldn't check broadcast loopback: {reason}",
                style::warn()
            );
        }
    }
    println!();
}

/// Result of [`probe_broadcast_loopback`].
enum ProbeResult {
    /// A magic UDP packet sent to `255.255.255.255:port` was received
    /// back by a local listener on `port`. Either the iptables NAT
    /// redirect is in place, or some other mechanism is looping the
    /// broadcast back. Either way, AMS2 will reach our listener.
    LoopbackWorks,
    /// The probe ran but nothing came back within the timeout — the
    /// kernel is dropping the locally-emitted broadcast.
    NoLoopback,
    /// We couldn't run the probe at all (port already bound, no
    /// broadcast-routable interface, etc.).
    CouldNotProbe(String),
}

/// Send one magic packet to the limited-broadcast address on `port` and
/// see if a local listener bound to `0.0.0.0:port` picks it up. Doesn't
/// require root. Filters by a unique magic payload so concurrent traffic
/// (an actually-running game) doesn't confuse the result.
fn probe_broadcast_loopback(port: u16) -> ProbeResult {
    const PROBE_MAGIC: &[u8] = b"moza-rev-loopback-probe-v1";
    const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

    let listener_addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), port);
    let listener = match UdpSocket::bind(listener_addr) {
        Ok(s) => s,
        Err(e) => {
            return ProbeResult::CouldNotProbe(format!(
                "couldn't bind 0.0.0.0:{port} ({e}); is moza-rev or another listener running?"
            ));
        }
    };
    if let Err(e) = listener.set_read_timeout(Some(PROBE_TIMEOUT)) {
        return ProbeResult::CouldNotProbe(format!("set_read_timeout: {e}"));
    }

    let sender = match UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0)) {
        Ok(s) => s,
        Err(e) => return ProbeResult::CouldNotProbe(format!("can't open sender socket: {e}")),
    };
    if let Err(e) = sender.set_broadcast(true) {
        return ProbeResult::CouldNotProbe(format!("set_broadcast: {e}"));
    }

    let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, port);
    if let Err(e) = sender.send_to(PROBE_MAGIC, dest) {
        return ProbeResult::CouldNotProbe(format!("send broadcast probe: {e}"));
    }

    // Drain the listener for the timeout window, ignoring anything that
    // isn't our magic (in case the game is actively broadcasting).
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        match listener.recv(&mut buf) {
            Ok(n) if &buf[..n] == PROBE_MAGIC => return ProbeResult::LoopbackWorks,
            Ok(_) => {} // some other packet — keep waiting
            Err(_) => return ProbeResult::NoLoopback,
        }
    }
    ProbeResult::NoLoopback
}

//
// BeamNG.drive — JSON config under XDG data, not Steam compatdata
//

/// `serde_json::Value::as_i64` returns i64, so widen the protocol's u16
/// at compile time rather than at every call site.
const BEAMNG_DEFAULT_PORT: i64 = outgauge::DEFAULT_PORT as i64;
const BEAMNG_DEFAULT_ADDRESS: &str = "127.0.0.1";

/// BeamNG.drive writes its settings outside the Steam Wine prefix, into the
/// host's XDG data dir (so they survive Proton prefix wipes). Look in both
/// the Flatpak Steam and the native paths.
fn beamng_cloud_settings() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let candidates = [
        home.join(".var/app/com.valvesoftware.Steam/.local/share/BeamNG/BeamNG.drive/current/settings/cloud/settings.json"),
        home.join(".local/share/BeamNG/BeamNG.drive/current/settings/cloud/settings.json"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn handle_beamng() -> io::Result<()> {
    println!("{}", style::title("BeamNG.drive"));
    let Some(settings_path) = beamng_cloud_settings() else {
        println!(
            "  Installed but no settings/cloud/settings.json yet — start the game once, then re-run.\n"
        );
        return Ok(());
    };

    let raw = fs::read_to_string(&settings_path)?;
    let mut doc: Value = serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    // Keys absent → defaults from BeamNG/settings/defaults.json.
    let current_enabled = doc
        .get("protocols_outgauge_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current_address = doc
        .get("protocols_outgauge_address")
        .and_then(Value::as_str)
        .unwrap_or(BEAMNG_DEFAULT_ADDRESS)
        .to_owned();
    let current_port = doc
        .get("protocols_outgauge_port")
        .and_then(Value::as_i64)
        .unwrap_or(BEAMNG_DEFAULT_PORT);

    println!("  config: {}", settings_path.display());
    println!(
        "  current: OutGauge enabled={current_enabled}, address={current_address}, port={current_port}"
    );

    if current_enabled {
        println!(
            "  {} OutGauge already enabled — no changes needed.",
            style::ok()
        );
        println!();
        return Ok(());
    }

    println!(
        "  proposed: protocols_outgauge_enabled = true (address/port unchanged: {current_address}:{current_port})"
    );
    println!(
        "  {} close BeamNG before applying — a running game may overwrite the change.",
        style::warn()
    );
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }

    doc["protocols_outgauge_enabled"] = Value::from(true);
    let new_raw = serde_json::to_string_pretty(&doc)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_with_backup(&settings_path, &new_raw)?;
    println!("  {} written.", style::ok());
    println!();
    Ok(())
}

//
// Assetto Corsa — UDP needs no in-game config; install-side fix-up only
//
// AC's UDP telemetry listens on port 9996 unconditionally once a session
// is loaded — no setting toggles it. The one install-side gotcha is
// that AC's stock launcher (AssettoCorsa.exe, .NET WPF + CEF3) often
// fails on current Wine/Proton. The standard workaround is to add
// `acs.exe` as a non-Steam shortcut, which then needs `steam_appid.txt`
// alongside the binary or acs.exe will exit ~2 seconds after start
// because its SteamAPI_Init can't reconcile the shortcut's hash-derived
// AppID with the real one.
//

fn handle_ac() -> io::Result<()> {
    let port = assetto_corsa::DEFAULT_PORT;
    println!("{}", style::title("Assetto Corsa"));
    println!("  UDP telemetry needs no in-game setup — port {port} is always listening");
    println!("  once a session is loaded.");

    let Some(install) = steam_install_path(AC_INSTALL_DIR) else {
        println!("  Couldn't locate install directory.\n");
        return Ok(());
    };

    let appid_file = install.join("steam_appid.txt");
    if appid_file.exists() {
        println!(
            "  {} steam_appid.txt present at {} — direct acs.exe launches will work.\n",
            style::ok(),
            appid_file.display()
        );
        return Ok(());
    }

    println!("  AC's stock launcher (AssettoCorsa.exe) often fails on current Wine/Proton with");
    println!("  a CEF3/.NET assembly load error. Workaround: add acs.exe as a non-Steam game");
    println!("  forced to a stable Proton. That direct launch then needs a steam_appid.txt");
    println!("  next to the binary, or acs.exe exits ~2s after start with no error message.");
    println!(
        "  proposed: write \"{AC_APP_ID}\" to {}",
        appid_file.display()
    );
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }
    fs::write(&appid_file, format!("{AC_APP_ID}\n"))?;
    println!("  {} written.\n", style::ok());
    Ok(())
}

//
// Assetto Corsa Competizione — UTF-16 LE JSON
//
// ACC writes Documents/Assetto Corsa Competizione/Config/broadcasting.json
// as UTF-16 LE without a BOM. Decoding to UTF-8, parsing JSON, then
// re-encoding UTF-16 LE preserves whatever ACC will accept on its next
// read. Broadcasting is local-only and the password's purpose is to
// stop accidental cross-tool collisions, so we propose a memorable
// default ("moza") rather than a random one.
//

fn handle_acc_competizione() -> io::Result<()> {
    println!("{}", style::title("Assetto Corsa Competizione"));
    let Some(docs) = proton_documents(ACC_APP_ID) else {
        println!(
            "  Installed but never launched — start the game once so it generates its config, then re-run.\n"
        );
        return Ok(());
    };
    let config_path = docs
        .join("Assetto Corsa Competizione")
        .join("Config")
        .join("broadcasting.json");
    if !config_path.exists() {
        println!("  Couldn't find {}.\n", config_path.display());
        return Ok(());
    }

    let bytes = fs::read(&config_path)?;
    let raw = decode_utf16_le(&bytes).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("UTF-16 LE decode: {e}"))
    })?;
    let mut doc: Value = serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let current_port = doc
        .get("updListenerPort")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let current_password = doc
        .get("connectionPassword")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    println!("  config: {}", config_path.display());
    println!(
        "  current: updListenerPort={current_port}, connectionPassword={}",
        if current_password.is_empty() {
            "(empty)".to_string()
        } else {
            format!("(set, {} chars)", current_password.chars().count())
        },
    );

    let mut updates: Vec<(&str, Value)> = Vec::new();
    if current_port != ACC_DEFAULT_PORT {
        updates.push(("updListenerPort", Value::from(ACC_DEFAULT_PORT)));
    }
    let proposed_password = if current_password.is_empty() {
        updates.push((
            "connectionPassword",
            Value::from(assetto_corsa_competizione::DEFAULT_PASSWORD),
        ));
        assetto_corsa_competizione::DEFAULT_PASSWORD.to_owned()
    } else {
        current_password.clone()
    };

    if updates.is_empty() {
        println!(
            "  {} broadcasting already enabled — no changes needed.",
            style::ok()
        );
        report_acc_probe(ACC_DEFAULT_PORT as u16, &proposed_password);
        return Ok(());
    }

    let proposed_summary: Vec<String> = updates.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("  proposed: {}", proposed_summary.join(", "));
    println!(
        "  {} close ACC before applying — a running game may overwrite the change.",
        style::warn()
    );
    if !confirm("  Apply?")? {
        println!("  skipped.");
        report_acc_probe(ACC_DEFAULT_PORT as u16, &proposed_password);
        return Ok(());
    }

    for (k, v) in updates {
        doc[k] = v;
    }
    let new_raw = json_to_acc_format(&doc)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let new_bytes = encode_utf16_le(&new_raw);
    write_with_backup(&config_path, &new_bytes)?;
    println!(
        "  {} written. Use --password \"{proposed_password}\" with the broadcasting client \
         (e.g. `cargo run --example assetto_corsa_competizione_log -- --password \"{proposed_password}\"`).",
        style::ok()
    );
    println!(
        "  {} restart ACC, then re-run `moza-rev configure` to verify the broadcasting port \
         is actually open.",
        style::warn()
    );
    println!();
    Ok(())
}

/// Live probe: send a real REGISTER packet to ACC's broadcasting port
/// and report what comes back. Distinguishes "ACC not listening" from
/// "ACC listening but rejecting our password" from "wedged" — useful
/// because ACC's broadcasting status has no in-game UI.
fn report_acc_probe(port: u16, password: &str) {
    println!("  probing 127.0.0.1:{port}…");
    match probe_acc_broadcasting(port, password) {
        AccProbeResult::Listening {
            connection_id,
            readonly,
        } => {
            println!(
                "  {} ACC responded: connection_id={connection_id}, readonly={readonly}. \
                 Broadcasting is live.",
                style::ok()
            );
        }
        AccProbeResult::Rejected { error } => {
            let hint = if error.to_lowercase().contains("outdated") {
                "Bump `assetto_corsa_competizione::PROTOCOL_VERSION` to match the version ACC expects."
            } else if error.to_lowercase().contains("password") {
                "The connectionPassword on disk doesn't match what's in ACC's memory — \
                 close ACC, re-run this command, then restart ACC."
            } else {
                "See the message above — moza-rev's REGISTER reached ACC but ACC refused it."
            };
            println!(
                "  {} ACC rejected our REGISTER: {error:?}.\n  {hint}",
                style::cross()
            );
        }
        AccProbeResult::Refused => {
            println!(
                "  {} nothing is listening on UDP/{port} — ACC isn't running, or it was \
                 launched before broadcasting.json was written. Start (or restart) ACC and \
                 re-run this command.",
                style::warn()
            );
        }
        AccProbeResult::Timeout => {
            println!(
                "  {} no reply within the probe window — ACC may be loading. \
                 Try again once it's at the main menu.",
                style::warn()
            );
        }
        AccProbeResult::ProbeError(reason) => {
            println!("  {} couldn't probe: {reason}", style::warn());
        }
    }
    println!();
}

enum AccProbeResult {
    Listening { connection_id: i32, readonly: bool },
    Rejected { error: String },
    Refused,
    Timeout,
    ProbeError(String),
}

fn probe_acc_broadcasting(port: u16, password: &str) -> AccProbeResult {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
    let target = format!("127.0.0.1:{port}");

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => return AccProbeResult::ProbeError(format!("bind ephemeral: {e}")),
    };
    if let Err(e) = socket.connect(&target) {
        return AccProbeResult::ProbeError(format!("connect {target}: {e}"));
    }
    if let Err(e) = socket.set_read_timeout(Some(PROBE_TIMEOUT)) {
        return AccProbeResult::ProbeError(format!("set_read_timeout: {e}"));
    }

    let pkt = assetto_corsa_competizione::build_register("moza-rev-verify", password, 1000, "");
    if let Err(e) = socket.send(&pkt) {
        if e.kind() == ErrorKind::ConnectionRefused {
            return AccProbeResult::Refused;
        }
        return AccProbeResult::ProbeError(format!("send register: {e}"));
    }

    let mut buf = [0u8; 4096];
    match socket.recv(&mut buf) {
        Ok(n) => match assetto_corsa_competizione::parse_message(&buf[..n]) {
            Some(assetto_corsa_competizione::Message::RegistrationResult(r)) if r.success => {
                // Best-effort cleanup so ACC doesn't keep us in its broadcaster list.
                let _ = socket.send(&assetto_corsa_competizione::build_unregister(
                    r.connection_id,
                ));
                AccProbeResult::Listening {
                    connection_id: r.connection_id,
                    readonly: r.readonly,
                }
            }
            Some(assetto_corsa_competizione::Message::RegistrationResult(r)) => {
                AccProbeResult::Rejected {
                    error: if r.error_message.is_empty() {
                        "(no error message)".to_string()
                    } else {
                        r.error_message
                    },
                }
            }
            _ => AccProbeResult::ProbeError(format!("unexpected reply ({n} bytes)")),
        },
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => AccProbeResult::Refused,
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
            ) =>
        {
            AccProbeResult::Timeout
        }
        Err(e) => AccProbeResult::ProbeError(format!("recv: {e}")),
    }
}

/// Serialize JSON in the exact shape ACC emits: 4-space indent, no
/// trailing newline, key order preserved from the loaded document.
/// stock `serde_json::to_string_pretty` uses 2-space indent and (with
/// default features) alphabetises keys — both differences make ACC's
/// loader decide our file "needs rewriting" and clobber it on launch.
fn json_to_acc_format(doc: &Value) -> serde_json::Result<String> {
    let formatter = PrettyFormatter::with_indent(b"    ");
    let mut buf = Vec::new();
    let mut ser = Serializer::with_formatter(&mut buf, formatter);
    doc.serialize(&mut ser)?;
    // PrettyFormatter only emits ASCII whitespace + JSON tokens, all valid UTF-8.
    Ok(String::from_utf8(buf).expect("PrettyFormatter emits valid UTF-8"))
}

/// Decode a UTF-16 LE byte stream (no BOM) to a Rust `String`. ACC's
/// broadcasting.json is the only file we touch in this format.
fn decode_utf16_le(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err(format!(
            "odd-length UTF-16 byte stream ({} bytes)",
            bytes.len()
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|e| e.to_string())
}

fn encode_utf16_le(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

//
// I/O helpers
//

fn write_with_backup(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    if path.exists() {
        let backup = path.with_extension(match path.extension().and_then(|s| s.to_str()) {
            Some(ext) => format!("{ext}.bak"),
            None => "bak".to_string(),
        });
        fs::copy(path, &backup)?;
        println!("  backup: {}", backup.display());
    }
    fs::write(path, content)
}

fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xml_attribute_values() {
        let line = r#"<udp enabled="true" extradata="3" ip="127.0.0.1" port="20777" delay="1" />"#;
        assert_eq!(parse_xml_attr(line, "enabled").as_deref(), Some("true"));
        assert_eq!(parse_xml_attr(line, "extradata").as_deref(), Some("3"));
        assert_eq!(parse_xml_attr(line, "port").as_deref(), Some("20777"));
        assert_eq!(parse_xml_attr(line, "missing"), None);
    }

    #[test]
    fn rewrites_only_listed_attributes() {
        let original =
            r#"<udp enabled="false" extradata="0" ip="127.0.0.1" port="20777" delay="1" />"#;
        let updated = rewrite_xml_attrs(original, &[("enabled", "true"), ("extradata", "3")]);
        assert!(updated.contains(r#"enabled="true""#));
        assert!(updated.contains(r#"extradata="3""#));
        // Untouched attrs preserved.
        assert!(updated.contains(r#"ip="127.0.0.1""#));
        assert!(updated.contains(r#"port="20777""#));
        assert!(updated.contains(r#"delay="1""#));
    }

    #[test]
    fn finds_udp_element_inside_motion_platform() {
        // Newer DR2-style XML.
        let xml = "<motion_platform>\n\t<dbox enabled=\"true\" />\n\t<udp enabled=\"false\" port=\"20777\" />\n</motion_platform>";
        let range = find_motion_or_udp_element(xml).unwrap();
        assert_eq!(
            &xml[range.clone()],
            "<udp enabled=\"false\" port=\"20777\" />"
        );
    }

    #[test]
    fn finds_motion_element_in_older_games() {
        // DiRT Showdown and other pre-DR1 Codemasters games.
        let xml = r#"<hardware_settings_config>
	<motion enabled="true" ip="dbox" port="20777" delay="1" extradata="0" />
</hardware_settings_config>"#;
        let range = find_motion_or_udp_element(xml).unwrap();
        assert_eq!(
            &xml[range.clone()],
            r#"<motion enabled="true" ip="dbox" port="20777" delay="1" extradata="0" />"#
        );
    }

    #[test]
    fn prefers_udp_over_motion_when_both_present() {
        // Defensive: if a future game has both elements, take <udp>.
        let xml = r#"<x><motion enabled="false" /><udp enabled="true" /></x>"#;
        let range = find_motion_or_udp_element(xml).unwrap();
        assert!(xml[range].starts_with("<udp "));
    }

    #[test]
    fn utf16_le_roundtrips_ascii() {
        let s = r#"{"updListenerPort": 9000}"#;
        let encoded = encode_utf16_le(s);
        // ASCII: every other byte is 0.
        assert_eq!(encoded.len(), s.len() * 2);
        assert_eq!(encoded[0], b'{');
        assert_eq!(encoded[1], 0);
        assert_eq!(decode_utf16_le(&encoded).unwrap(), s);
    }

    #[test]
    fn utf16_le_decodes_acc_broadcasting_json_sample() {
        // Reproduces the exact bytes from a fresh ACC broadcasting.json
        // (UTF-16 LE, no BOM, 4-space indent, no trailing newline).
        // ACC's literal key spelling is `updListenerPort` — `upd` not
        // `udp`, a Kunos typo. Bytes here encode that exact spelling.
        let bytes: &[u8] = &[
            0x7b, 0x00, 0x0a, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00,
            0x75, 0x00, 0x70, 0x00, 0x64, 0x00, 0x4c, 0x00, 0x69, 0x00, 0x73, 0x00, 0x74, 0x00,
            0x65, 0x00, 0x6e, 0x00, 0x65, 0x00, 0x72, 0x00, 0x50, 0x00, 0x6f, 0x00, 0x72, 0x00,
            0x74, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x30, 0x00, 0x7d, 0x00,
        ];
        let s = decode_utf16_le(bytes).unwrap();
        assert_eq!(s, "{\n    \"updListenerPort\": 0}");
        // serde_json must accept the decoded form even with the leading newline.
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["updListenerPort"], 0);
    }

    #[test]
    fn utf16_le_encode_then_parse_succeeds() {
        let v = serde_json::json!({
            "updListenerPort": 9000,
            "connectionPassword": "moza",
            "commandPassword": "",
        });
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        let bytes = encode_utf16_le(&pretty);
        let decoded = decode_utf16_le(&bytes).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(reparsed["updListenerPort"], 9000);
        assert_eq!(reparsed["connectionPassword"], "moza");
    }

    #[test]
    fn utf16_le_rejects_odd_length() {
        assert!(decode_utf16_le(&[0x7b, 0x00, 0x0a]).is_err());
    }

    #[test]
    fn json_to_acc_format_matches_acc_layout() {
        // Mirror the exact key order ACC uses on disk. With the
        // `preserve_order` feature, parsing then re-emitting keeps the
        // order stable.
        let raw = "{\n    \"updListenerPort\": 0,\n    \"connectionPassword\": \"\",\n    \"commandPassword\": \"\"\n}";
        let mut doc: Value = serde_json::from_str(raw).unwrap();
        doc["updListenerPort"] = Value::from(9000);
        doc["connectionPassword"] = Value::from("moza");
        let out = json_to_acc_format(&doc).unwrap();
        let expected = "{\n    \"updListenerPort\": 9000,\n    \"connectionPassword\": \"moza\",\n    \"commandPassword\": \"\"\n}";
        assert_eq!(out, expected);
    }

    #[test]
    fn end_to_end_acc_file_path_does_not_duplicate_keys() {
        // Exact bytes from the user's broadcasting.json before our fix:
        // 178 bytes UTF-16 LE, no BOM, port=0 password="moza" cmd="".
        let input: &[u8] = &[
            0x7b, 0x00, 0x0a, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00,
            0x75, 0x00, 0x70, 0x00, 0x64, 0x00, 0x4c, 0x00, 0x69, 0x00, 0x73, 0x00, 0x74, 0x00,
            0x65, 0x00, 0x6e, 0x00, 0x65, 0x00, 0x72, 0x00, 0x50, 0x00, 0x6f, 0x00, 0x72, 0x00,
            0x74, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x30, 0x00, 0x2c, 0x00, 0x0a, 0x00,
            0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00, 0x63, 0x00, 0x6f, 0x00,
            0x6e, 0x00, 0x6e, 0x00, 0x65, 0x00, 0x63, 0x00, 0x74, 0x00, 0x69, 0x00, 0x6f, 0x00,
            0x6e, 0x00, 0x50, 0x00, 0x61, 0x00, 0x73, 0x00, 0x73, 0x00, 0x77, 0x00, 0x6f, 0x00,
            0x72, 0x00, 0x64, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x22, 0x00, 0x6d, 0x00,
            0x6f, 0x00, 0x7a, 0x00, 0x61, 0x00, 0x22, 0x00, 0x2c, 0x00, 0x0a, 0x00, 0x20, 0x00,
            0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00, 0x63, 0x00, 0x6f, 0x00, 0x6d, 0x00,
            0x6d, 0x00, 0x61, 0x00, 0x6e, 0x00, 0x64, 0x00, 0x50, 0x00, 0x61, 0x00, 0x73, 0x00,
            0x73, 0x00, 0x77, 0x00, 0x6f, 0x00, 0x72, 0x00, 0x64, 0x00, 0x22, 0x00, 0x3a, 0x00,
            0x20, 0x00, 0x22, 0x00, 0x22, 0x00, 0x0a, 0x00, 0x7d, 0x00,
        ];
        let raw = decode_utf16_le(input).unwrap();
        let mut doc: Value = serde_json::from_str(&raw).unwrap();
        doc["updListenerPort"] = Value::from(9000_i64);
        let out = json_to_acc_format(&doc).unwrap();
        // Must contain exactly one ListenerPort entry — searching for any
        // case of `ListenerPort` catches both correct and any wrongly-cased
        // sibling key, so we'd notice a duplicate either way.
        let count = out.matches("ListenerPort").count();
        assert_eq!(count, 1, "got duplicates / wrong key:\n{out}");
        assert!(out.contains("\"updListenerPort\": 9000"));
    }

    #[test]
    fn json_to_acc_format_roundtrips_to_utf16_le_bytes_acc_emits() {
        // Build the value in the order ACC writes, encode via our pipeline,
        // and verify the resulting UTF-16 LE bytes match a stock ACC file
        // (port=0, empty passwords) byte-for-byte.
        let raw = "{\n    \"updListenerPort\": 0,\n    \"connectionPassword\": \"\",\n    \"commandPassword\": \"\"\n}";
        let doc: Value = serde_json::from_str(raw).unwrap();
        let out = json_to_acc_format(&doc).unwrap();
        let bytes = encode_utf16_le(&out);
        // Mirror the xxd of an unmodified broadcasting.json (178 bytes).
        let expected: &[u8] = &[
            0x7b, 0x00, 0x0a, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00,
            0x75, 0x00, 0x70, 0x00, 0x64, 0x00, 0x4c, 0x00, 0x69, 0x00, 0x73, 0x00, 0x74, 0x00,
            0x65, 0x00, 0x6e, 0x00, 0x65, 0x00, 0x72, 0x00, 0x50, 0x00, 0x6f, 0x00, 0x72, 0x00,
            0x74, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x30, 0x00, 0x2c, 0x00, 0x0a, 0x00,
            0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00, 0x63, 0x00, 0x6f, 0x00,
            0x6e, 0x00, 0x6e, 0x00, 0x65, 0x00, 0x63, 0x00, 0x74, 0x00, 0x69, 0x00, 0x6f, 0x00,
            0x6e, 0x00, 0x50, 0x00, 0x61, 0x00, 0x73, 0x00, 0x73, 0x00, 0x77, 0x00, 0x6f, 0x00,
            0x72, 0x00, 0x64, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x22, 0x00, 0x22, 0x00,
            0x2c, 0x00, 0x0a, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x22, 0x00,
            0x63, 0x00, 0x6f, 0x00, 0x6d, 0x00, 0x6d, 0x00, 0x61, 0x00, 0x6e, 0x00, 0x64, 0x00,
            0x50, 0x00, 0x61, 0x00, 0x73, 0x00, 0x73, 0x00, 0x77, 0x00, 0x6f, 0x00, 0x72, 0x00,
            0x64, 0x00, 0x22, 0x00, 0x3a, 0x00, 0x20, 0x00, 0x22, 0x00, 0x22, 0x00, 0x0a, 0x00,
            0x7d, 0x00,
        ];
        assert_eq!(bytes, expected, "byte-level output drift from ACC's format");
    }
}
