use std::env;
use std::io::Write;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use log::{error, info};

use moza_rev::moza::{self, Moza, Protocol};
use moza_rev::wreckfest::{self, EngineState};

const DEFAULT_PORT: u16 = 23123;
const DEFAULT_LED_COUNT: usize = 10;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = match Args::from_env() {
        Ok(a) => a,
        Err(msg) => {
            error!("{msg}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let bind_addr = format!("0.0.0.0:{}", args.port);
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind {bind_addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!("listening for Wreckfest 2 telemetry on udp://{bind_addr}");

    let serial_path = match args.serial_path.or_else(moza::find_wheelbase) {
        Some(p) => p,
        None => {
            error!(
                "no Moza wheelbase found under /dev/serial/by-id/ (looking for *Gudsen*Base*). \
                 plug it in, or pass --serial /dev/ttyACMx"
            );
            return ExitCode::FAILURE;
        }
    };
    let protocol = args.protocol.unwrap_or_else(|| moza::detect_protocol(&serial_path));
    info!("opening Moza wheelbase at {serial_path} ({protocol:?} protocol)");
    let mut wheel = match Moza::open(&serial_path, protocol) {
        Ok(m) => m,
        Err(e) => {
            error!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut buf = vec![0u8; 2048];
    let mut last_bitmask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();
    let mut packets_since_status: u32 = 0;

    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(engine) = wreckfest::parse_main(&buf[..n]) else {
            continue;
        };
        packets_since_status += 1;

        let bitmask = rpm_to_bitmask(&engine, args.led_count);

        let changed = Some(bitmask) != last_bitmask;
        let stale = last_send.elapsed() >= HEARTBEAT_INTERVAL;
        if changed || stale {
            if let Err(e) = wheel.send_rpm_bitmask(bitmask, args.led_count) {
                error!("write to wheel: {e}");
                continue;
            }
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        if args.verbose {
            print_status(&engine, bitmask, args.led_count, packets_since_status, true);
            packets_since_status = 0;
        } else if last_status.elapsed() >= STATUS_INTERVAL {
            print_status(&engine, bitmask, args.led_count, packets_since_status, false);
            last_status = Instant::now();
            packets_since_status = 0;
        }
    }
}

fn print_status(engine: &EngineState, bitmask: u32, led_count: usize, packets: u32, verbose: bool) {
    let bar = led_bar(bitmask, led_count);
    let line = format!(
        "rpm {:>5} / {:>5}  idle {:>4}  [{}]  {} pkt",
        engine.rpm, engine.rpm_redline, engine.rpm_idle, bar, packets
    );
    if verbose {
        println!("{line}");
    } else {
        // Overwrite a single status line in place.
        print!("\r{line}    ");
        let _ = std::io::stdout().flush();
    }
}

fn led_bar(bitmask: u32, led_count: usize) -> String {
    (0..led_count)
        .map(|i| if bitmask & (1 << i) != 0 { '●' } else { '○' })
        .collect()
}

fn rpm_to_bitmask(engine: &EngineState, led_count: usize) -> u32 {
    if engine.rpm_redline <= engine.rpm_idle || led_count == 0 {
        return 0;
    }
    let span = (engine.rpm_redline - engine.rpm_idle) as f32;
    let above_idle = (engine.rpm - engine.rpm_idle).max(0) as f32;
    let frac = (above_idle / span).clamp(0.0, 1.0);
    let lit = (frac * led_count as f32).round() as usize;
    let lit = lit.min(led_count);
    if lit == 0 {
        0
    } else if lit >= 32 {
        u32::MAX
    } else {
        (1u32 << lit) - 1
    }
}

struct Args {
    port: u16,
    serial_path: Option<String>,
    led_count: usize,
    protocol: Option<Protocol>,
    verbose: bool,
}

impl Args {
    fn from_env() -> Result<Self, String> {
        let mut port = DEFAULT_PORT;
        let mut serial_path = None;
        let mut led_count = DEFAULT_LED_COUNT;
        let mut protocol = None;
        let mut verbose = false;

        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--port" | "-p" => {
                    let v = it.next().ok_or("--port needs a value")?;
                    port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--serial" | "-s" => {
                    serial_path = Some(it.next().ok_or("--serial needs a path")?);
                }
                "--leds" | "-l" => {
                    let v = it.next().ok_or("--leds needs a value")?;
                    led_count = v.parse().map_err(|_| format!("invalid led count: {v}"))?;
                }
                "--protocol" => {
                    let v = it.next().ok_or("--protocol needs a value")?;
                    protocol = Some(match v.as_str() {
                        "modern" | "new" => Protocol::Modern,
                        "legacy" | "old" | "es" => Protocol::Legacy,
                        other => return Err(format!("invalid protocol (expected modern|legacy): {other}")),
                    });
                }
                "--verbose" | "-v" => {
                    verbose = true;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Self {
            port,
            serial_path,
            led_count,
            protocol,
            verbose,
        })
    }
}

fn print_usage() {
    eprintln!(
        "moza-rev — drive the Moza wheel's RPM LED bar from Wreckfest 2 telemetry\n\
         \n\
         usage: moza-rev [--port PORT] [--serial /dev/ttyACMx] [--leds N] [--protocol modern|legacy] [-v]\n\
         \n\
         defaults: --port {DEFAULT_PORT}, --leds {DEFAULT_LED_COUNT}, serial+protocol autodetected.\n\
         R3 / R5 bases default to legacy; everything else to modern.\n\
         \n\
         Wreckfest 2 telemetry must be enabled first. Set \"enabled\": 1 in:\n\
           ~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/\n\
             compatdata/1203190/pfx/drive_c/users/steamuser/Documents/My Games/\n\
             Wreckfest 2/<userid>/savegame/telemetry/config.json"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(rpm: i32) -> EngineState {
        EngineState {
            rpm,
            rpm_redline: 6500,
            rpm_idle: 800,
        }
    }

    #[test]
    fn below_idle_lights_nothing() {
        assert_eq!(rpm_to_bitmask(&engine(500), 10), 0);
    }

    #[test]
    fn at_redline_lights_everything() {
        assert_eq!(rpm_to_bitmask(&engine(6500), 10), 0x3FF);
    }

    #[test]
    fn above_redline_clamps() {
        assert_eq!(rpm_to_bitmask(&engine(9000), 10), 0x3FF);
    }

    #[test]
    fn midband_lights_half() {
        // (3650 - 800) / (6500 - 800) = 0.5 → 5 of 10 LEDs
        assert_eq!(rpm_to_bitmask(&engine(3650), 10), 0x1F);
    }

    #[test]
    fn unset_engine_data_is_safe() {
        let zero = EngineState {
            rpm: 0,
            rpm_redline: 0,
            rpm_idle: 0,
        };
        assert_eq!(rpm_to_bitmask(&zero, 10), 0);
    }
}
