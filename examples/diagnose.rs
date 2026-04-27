// Diagnose: read settings off the Moza wheelbase and print them. Confirms
// bidirectional comms (with checksum-validated frames) and shows what mode
// the wheel is in so we know whether the "all on" / "breathing" pattern is
// configured or default state.
//
// Run:  cargo run --example diagnose
//       MOZA_TRACE=1 cargo run --example diagnose
//
// Settings read (ES wheel):
//   rpm-indicator-mode  cmd [04]    1=RPM/telemetry, 2=Off, 3=On(demo)
//   rpm-mode            cmd [17]    0=Percent, 1=RPM
//   rpm-display-mode    cmd [08]    1=Mode 1, 2=Mode 2
//   rpm-brightness      cmd [1B,00,FF]  0..100 (modern) / 0..15 (ES via 14,00)
//   wheel id check      via reading any of the above

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use moza_rev::moza::{self, GROUP_WHEEL_READ, Moza, Protocol};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut serial_override: Option<String> = None;
    let mut protocol_override: Option<Protocol> = None;

    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--serial" => serial_override = it.next(),
            "--protocol" => {
                protocol_override = it.next().and_then(|v| match v.as_str() {
                    "modern" | "new" => Some(Protocol::Modern),
                    "legacy" | "old" | "es" => Some(Protocol::Legacy),
                    _ => None,
                });
            }
            "--help" | "-h" => {
                eprintln!("usage: diagnose [--serial PATH] [--protocol modern|legacy]");
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }

    let serial_path = match serial_override.or_else(moza::find_wheelbase) {
        Some(p) => p,
        None => {
            eprintln!("no Moza wheelbase found");
            return ExitCode::FAILURE;
        }
    };
    let protocol = protocol_override.unwrap_or_else(|| moza::detect_protocol(&serial_path));
    println!("opening {serial_path} ({protocol:?})");
    let mut wheel = match Moza::open(&serial_path, protocol) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let deadline = Duration::from_millis(800);

    println!();
    read_one(
        &mut wheel,
        "rpm-indicator-mode",
        &[0x04],
        1,
        deadline,
        |v| match v {
            1 => "RPM (telemetry-driven)".to_string(),
            2 => "Off".to_string(),
            3 => "On (demo / always lit)".to_string(),
            other => format!("unknown ({other})"),
        },
    );
    read_one(
        &mut wheel,
        "rpm-mode (es)",
        &[0x17],
        1,
        deadline,
        |v| match v {
            0 => "Percent".to_string(),
            1 => "RPM".to_string(),
            other => format!("unknown ({other})"),
        },
    );
    read_one(
        &mut wheel,
        "rpm-display-mode",
        &[0x08],
        1,
        deadline,
        |v| match v {
            1 => "Mode 1".to_string(),
            2 => "Mode 2".to_string(),
            other => format!("unknown ({other})"),
        },
    );
    read_one(
        &mut wheel,
        "rpm-brightness (es)",
        &[0x14, 0x00],
        1,
        deadline,
        |v| format!("{v} / 15"),
    );

    println!();
    println!("(if all reads time out, the wheel ID is probably 0x15 not 0x17,");
    println!(" or another process is holding the port — try `lsof /dev/ttyACM0`)");

    ExitCode::SUCCESS
}

fn read_one(
    wheel: &mut Moza,
    name: &str,
    cmd_id: &[u8],
    response_len: usize,
    deadline: Duration,
    decode: impl Fn(u32) -> String,
) {
    match wheel.read_setting(GROUP_WHEEL_READ, cmd_id, response_len, deadline) {
        Ok(Some(bytes)) => {
            let v = bytes.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32);
            println!("{name:<24}  raw={bytes:02X?}  → {}", decode(v));
        }
        Ok(None) => println!("{name:<24}  no response"),
        Err(e) => println!("{name:<24}  error: {e}"),
    }
}
