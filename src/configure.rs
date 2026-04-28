// `moza-rev configure` — detect installed racing games and offer to
// enable their UDP telemetry output. Read-only by default; any file
// edit prompts for `y/N` confirmation and writes a `.bak` backup first.

use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

/// Steam app ids for games we know how to handle (or recognize).
const WF2_APP_ID: &str = "1203190";
const DR2_APP_ID: &str = "690790";
const DIRT_SHOWDOWN_APP_ID: &str = "65750";

/// Detect-only — these run on EGO/proprietary engines without native UDP
/// telemetry; the only path is memory injection (SpaceMonkey on Windows).
const NO_TELEMETRY_GAMES: &[(&str, &str)] = &[("228380", "Wreckfest"), ("1097130", "DIRT 5")];

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

/// Check whether the game's directory exists under any Steam library's
/// `steamapps/common`. The directory name is the case-sensitive label
/// Steam uses (e.g. "Wreckfest 2", "DiRT Rally 2.0").
fn game_installed(common_dir_name: &str) -> bool {
    steam_library_roots()
        .into_iter()
        .any(|root| root.join("steamapps/common").join(common_dir_name).exists())
}

pub fn run() -> ExitCode {
    println!("moza-rev configure: scanning for installed racing games\n");

    let mut any_handled = false;

    if game_installed("Wreckfest 2") {
        any_handled = true;
        if let Err(e) = handle_wf2() {
            eprintln!("Wreckfest 2: error: {e}\n");
        }
    }
    if game_installed("DiRT Rally 2.0") {
        any_handled = true;
        if let Err(e) = handle_dr2_style("DiRT Rally 2.0", DR2_APP_ID, "DiRT Rally 2.0") {
            eprintln!("DiRT Rally 2.0: error: {e}\n");
        }
    }
    if game_installed("DiRT Showdown") {
        any_handled = true;
        if let Err(e) = handle_dr2_style("DiRT Showdown", DIRT_SHOWDOWN_APP_ID, "DiRT Showdown") {
            eprintln!("DiRT Showdown: error: {e}\n");
        }
    }
    if game_installed("BeamNG.drive") {
        any_handled = true;
        handle_beamng();
    }
    for (app_id, name) in NO_TELEMETRY_GAMES {
        if game_installed(name) || proton_documents(app_id).is_some() {
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
    let Some(udp_line_range) = find_udp_element_range(&raw) else {
        println!("  No <udp .../> element found inside <motion_platform>. Leaving alone.\n");
        return Ok(());
    };
    let current_line = &raw[udp_line_range.clone()];
    let current_enabled = parse_xml_attr(current_line, "enabled");
    let current_extradata = parse_xml_attr(current_line, "extradata");
    let current_port = parse_xml_attr(current_line, "port");

    println!("  config: {}", config_path.display());
    println!(
        "  current: enabled={}, extradata={}, port={}",
        current_enabled.as_deref().unwrap_or("?"),
        current_extradata.as_deref().unwrap_or("?"),
        current_port.as_deref().unwrap_or("?"),
    );

    let needs_change =
        current_enabled.as_deref() != Some("true") || current_extradata.as_deref() != Some("3");
    if !needs_change {
        println!("  ✓ telemetry already enabled with extradata=3 — no changes needed.\n");
        return Ok(());
    }

    let new_line = rewrite_udp_attrs(current_line, &[("enabled", "true"), ("extradata", "3")]);
    println!("  proposed:\n    -{current_line}\n    +{new_line}");
    if !confirm("  Apply?")? {
        println!("  skipped.\n");
        return Ok(());
    }

    let mut new_raw = String::with_capacity(raw.len());
    new_raw.push_str(&raw[..udp_line_range.start]);
    new_raw.push_str(&new_line);
    new_raw.push_str(&raw[udp_line_range.end..]);
    write_with_backup(&config_path, &new_raw)?;
    println!("  ✓ written.\n");
    Ok(())
}

/// Locate `<udp ... />` (anywhere in the XML — game has only one).
fn find_udp_element_range(xml: &str) -> Option<std::ops::Range<usize>> {
    let start = xml.find("<udp ")?;
    let rel_end = xml[start..].find("/>")?;
    Some(start..start + rel_end + 2)
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
fn rewrite_udp_attrs(element: &str, updates: &[(&str, &str)]) -> String {
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
// BeamNG.drive
//

fn handle_beamng() {
    println!("BeamNG.drive");
    println!("  OutGauge config lives in BeamNG's user data directory and isn't");
    println!("  a single attribute we can confidently flip from outside the game.");
    println!("  Enable it manually:");
    println!("    Options → Other → Protocols → OutGauge: ip 127.0.0.1, port 4444, enable");
    println!("  Note: moza-rev's main listener doesn't yet consume OutGauge frames.\n");
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
        let updated = rewrite_udp_attrs(original, &[("enabled", "true"), ("extradata", "3")]);
        assert!(updated.contains(r#"enabled="true""#));
        assert!(updated.contains(r#"extradata="3""#));
        // Untouched attrs preserved.
        assert!(updated.contains(r#"ip="127.0.0.1""#));
        assert!(updated.contains(r#"port="20777""#));
        assert!(updated.contains(r#"delay="1""#));
    }

    #[test]
    fn finds_udp_element_inside_motion_platform() {
        let xml = "<motion_platform>\n\t<dbox enabled=\"true\" />\n\t<udp enabled=\"false\" port=\"20777\" />\n</motion_platform>";
        let range = find_udp_element_range(xml).unwrap();
        assert_eq!(
            &xml[range.clone()],
            "<udp enabled=\"false\" port=\"20777\" />"
        );
    }
}
