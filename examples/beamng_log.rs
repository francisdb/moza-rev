// Listen for BeamNG.drive OutGauge UDP telemetry and log a one-line
// summary per packet. OutGauge is the LFS-compatible dashboard protocol
// — BeamNG's implementation lives in
// `lua/vehicle/protocols/outgauge.lua` and is wire-identical to LFS's
// `OutGauge` so this example also works with Live For Speed.
//
// Enable in BeamNG.drive:
//   Options → Other → Protocols → OutGauge: enable, ip 127.0.0.1, port 4444
// Or run `cargo run -- configure` and accept the prompt.
//
// Run:
//   cargo run --example beamng_log
//   cargo run --example beamng_log -- --port 4444

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;

use log::{error, info};

use moza_rev::outgauge::{DEFAULT_PORT, Packet, dl, og};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut port = DEFAULT_PORT;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                let v = it.next().unwrap_or_default();
                port = match v.parse() {
                    Ok(p) => p,
                    Err(_) => {
                        error!("invalid port: {v}");
                        return ExitCode::from(2);
                    }
                };
            }
            "--help" | "-h" => {
                eprintln!("usage: beamng_log [--port PORT]");
                return ExitCode::SUCCESS;
            }
            other => {
                error!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }

    let bind_addr = format!("0.0.0.0:{port}");
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind {bind_addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!("listening for BeamNG OutGauge telemetry on udp://{bind_addr}");

    let mut buf = vec![0u8; 256];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(p) = Packet::from_bytes(&buf[..n]) else {
            info!("short packet ({} bytes) — ignoring", n);
            continue;
        };
        log_packet(&p);
    }
}

fn log_packet(p: &Packet) {
    let mut lights: Vec<&str> = Vec::new();
    let on = p.show_lights;
    if on & dl::SHIFT != 0 {
        lights.push("SHIFT");
    }
    if on & dl::HANDBRAKE != 0 {
        lights.push("HBRAKE");
    }
    if on & dl::PITSPEED != 0 {
        lights.push("PIT");
    }
    if on & dl::OILWARN != 0 {
        lights.push("OIL");
    }
    if on & dl::BATTERY != 0 {
        lights.push("BAT");
    }
    if on & dl::ABS != 0 {
        lights.push("ABS");
    }
    if on & dl::TC != 0 {
        lights.push("TC");
    }
    if on & dl::FULLBEAM != 0 {
        lights.push("HIGHBEAM");
    }
    if on & dl::SIGNAL_L != 0 {
        lights.push("←");
    }
    if on & dl::SIGNAL_R != 0 {
        lights.push("→");
    }
    let lights_str = if lights.is_empty() {
        String::new()
    } else {
        format!(" [{}]", lights.join(" "))
    };
    let turbo = if p.flags & og::TURBO != 0 {
        format!(" turbo={:.2}bar", { p.turbo })
    } else {
        String::new()
    };

    info!(
        "car={} rpm={:>5.0} gear={} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h fuel={:>3.0}% engT={:>3.0}°C{turbo}{lights_str}",
        p.car_name(),
        { p.rpm },
        p.gear_label(),
        { p.throttle * 100.0 },
        { p.brake * 100.0 },
        { p.clutch * 100.0 },
        p.speed_kmh(),
        { p.fuel * 100.0 },
        { p.eng_temp },
    );
}
