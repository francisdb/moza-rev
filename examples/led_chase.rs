// Diagnostic: drive the RPM bar so we can see whether our protocol writes
// reach the wheel at all.
//
// Modern wheels (CS Pro / KS Pro / FSR / R9+): single LED chase via bitmask.
// ES (R3/R5) wheels: walk an RPM-percent value 0..=1023; the wheel's own
// threshold table decides how many LEDs to light at each value (defaults
// land around 75% for first LED, 94% for last in Percent mode).
//
// Run:
//   cargo run --example led_chase
//   cargo run --example led_chase -- --leds 10 --step-ms 100
//   cargo run --example led_chase -- --protocol modern
//   MOZA_TRACE=1 cargo run --example led_chase

use std::env;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use log::{error, info};

use moza_rev::moza::{self, Moza, Protocol};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut led_count: usize = 10;
    let mut step_ms: u64 = 100;
    let mut protocol_override: Option<Protocol> = None;
    let mut serial_override: Option<String> = None;

    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--leds" => led_count = it.next().and_then(|v| v.parse().ok()).unwrap_or(10),
            "--step-ms" => step_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(100),
            "--protocol" => {
                protocol_override = it.next().and_then(|v| match v.as_str() {
                    "modern" | "new" => Some(Protocol::Modern),
                    "legacy" | "old" | "es" => Some(Protocol::Legacy),
                    _ => None,
                });
            }
            "--serial" => serial_override = it.next(),
            "--help" | "-h" => {
                eprintln!("usage: led_chase [--leds N] [--step-ms MS] [--protocol modern|legacy] [--serial PATH]");
                return ExitCode::SUCCESS;
            }
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    let serial_path = match serial_override.or_else(moza::find_wheelbase) {
        Some(p) => p,
        None => {
            eprintln!("no Moza wheelbase found under /dev/serial/by-id/");
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

    info!("press Ctrl+C to stop");

    let _ = protocol; // both protocols use the same chase pattern below
    run_chase(&mut wheel, led_count, step_ms)
}

// Mirror boxflat's `_wheel_rpm_test` (boxflat/panels/wheel_old.py:408): walk
// a single lit LED across the bar, then a cumulative fill, then a cumulative
// drain. The u32 payload is interpreted as a direct LED bitmask by the wheel
// firmware on both Legacy (R3/R5 ES) and Modern wheels.
fn run_chase(wheel: &mut Moza, led_count: usize, step_ms: u64) -> ExitCode {
    info!("chase across {led_count} LEDs, {step_ms}ms/step");
    let step = Duration::from_millis(step_ms);
    loop {
        // Single LED walking 0..N..0
        for i in 0..led_count {
            if let Err(e) = send(wheel, 1u32 << i, led_count) {
                error!("write failed: {e}");
                return ExitCode::FAILURE;
            }
            thread::sleep(step);
        }
        for i in (0..led_count.saturating_sub(1)).rev() {
            if let Err(e) = send(wheel, 1u32 << i, led_count) {
                error!("write failed: {e}");
                return ExitCode::FAILURE;
            }
            thread::sleep(step);
        }
        // Cumulative fill 0..N
        for i in 0..led_count {
            let bm = (1u32 << (i + 1)) - 1;
            if let Err(e) = send(wheel, bm, led_count) {
                error!("write failed: {e}");
                return ExitCode::FAILURE;
            }
            thread::sleep(step);
        }
        // Cumulative drain N..0
        for i in (0..led_count).rev() {
            let bm = if i == 0 { 0 } else { (1u32 << i) - 1 };
            if let Err(e) = send(wheel, bm, led_count) {
                error!("write failed: {e}");
                return ExitCode::FAILURE;
            }
            thread::sleep(step);
        }
    }
}

fn send(wheel: &mut Moza, bitmask: u32, led_count: usize) -> std::io::Result<()> {
    log::debug!("bitmask 0x{bitmask:08X}");
    wheel.send_rpm_bitmask(bitmask, led_count)
}
