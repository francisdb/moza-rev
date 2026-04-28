// `moza-rev configure` — detect installed racing games and offer to
// enable their UDP telemetry output. Read-only by default; any file
// edit prompts for `y/N` confirmation and writes a `.bak` backup first.

use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

/// Steam app ids for games we know how to handle (or recognize).
const WF2_APP_ID: &str = "1203190";
const DR2_APP_ID: &str = "690790";
const DIRT_SHOWDOWN_APP_ID: &str = "201700";
const BEAMNG_APP_ID: &str = "284160";

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
        app_id: "244210",
        name: "Assetto Corsa",
        notes: "  Has UDP remote telemetry (handshake protocol on ports 9996/9997).\n  \
                Enable in-game via Apps menu, or edit Documents/Assetto Corsa/cfg/.\n  \
                moza-rev does not yet have an AC parser.",
    },
    ManualEntry {
        app_id: "805550",
        name: "Assetto Corsa Competizione",
        notes: "  Has UDP Broadcasting API (port 9000, password handshake — connection-oriented).\n  \
                Edit Documents/Assetto Corsa Competizione/Config/broadcasting.json:\n  \
                  udpListenerPort: 9000, connectionPassword: <set this>.\n  \
                moza-rev does not yet have an ACC parser.",
    },
    ManualEntry {
        app_id: "3917090",
        name: "Assetto Corsa Rally",
        notes: "  Has UDP remote telemetry (same format as base Assetto Corsa).\n  \
                Some fields are not yet populated in early access.\n  \
                moza-rev does not yet have an AC-family parser.",
    },
    ManualEntry {
        app_id: "1066890",
        name: "Automobilista 2",
        notes: "  Has UDP telemetry on port 5606 using the Project CARS 2 format.\n  \
                In game: Options → System → UDP Protocol Version: Project CARS 2,\n  \
                                          Shared Memory: Project CARS 2,\n  \
                                          UDP Frequency: 1+\n  \
                moza-rev does not yet have a Madness-engine parser.",
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
    println!("moza-rev configure: scanning for installed racing games\n");

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
    if game_installed(BEAMNG_APP_ID) {
        any_handled = true;
        if let Err(e) = handle_beamng() {
            eprintln!("BeamNG.drive: error: {e}\n");
        }
    }
    for entry in MANUAL_TELEMETRY_GAMES {
        if game_installed(entry.app_id) {
            any_handled = true;
            println!("{}", entry.name);
            println!("{}\n", entry.notes);
        }
    }
    for (app_id, name) in NO_TELEMETRY_GAMES {
        if game_installed(app_id) {
            any_handled = true;
            println!("{name}");
            println!("  Detected, but the game has no native UDP telemetry.");
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
    println!("Wreckfest 2");
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
        println!("  ✓ telemetry already enabled — no changes needed.\n");
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
    println!("  ✓ written.\n");
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
    println!("{name}");
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
        println!("  ✓ telemetry already enabled with extradata=3 — no changes needed.\n");
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
    println!("  ✓ written.\n");
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
// BeamNG.drive — JSON config under XDG data, not Steam compatdata
//

const BEAMNG_DEFAULT_PORT: i64 = 4444;
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
    println!("BeamNG.drive");
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
        println!("  ✓ OutGauge already enabled — no changes needed.");
        println!();
        return Ok(());
    }

    println!(
        "  proposed: protocols_outgauge_enabled = true (address/port unchanged: {current_address}:{current_port})"
    );
    println!("  ⚠ close BeamNG before applying — a running game may overwrite the change.");
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }

    doc["protocols_outgauge_enabled"] = Value::from(true);
    let new_raw = serde_json::to_string_pretty(&doc)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_with_backup(&settings_path, &new_raw)?;
    println!("  ✓ written.");
    println!();
    Ok(())
}

//
// I/O helpers
//

fn write_with_backup(path: &Path, content: &str) -> io::Result<()> {
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
}
