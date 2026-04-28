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
use std::ptr;

use log::{error, info};

const DEFAULT_PORT: u16 = 4444;
/// Packet size without the optional trailing `id` field (LFS classic).
const MIN_PACKET_BYTES: usize = 92;
/// Packet size with the trailing `id` field (BeamNG default).
const FULL_PACKET_BYTES: usize = std::mem::size_of::<OutGaugePacket>(); // 96

// OG_x flag bits (`flags` field). OG_KM (16384) and OG_BAR (32768) only
// affect display unit preferences — both are always set by BeamNG and we
// don't currently surface them.
const OG_TURBO: u16 = 8192; // turbo gauge meaningful

// DL_x bits (`dashLights` / `showLights`).
const DL_SHIFT: u32 = 1 << 0;
const DL_FULLBEAM: u32 = 1 << 1;
const DL_HANDBRAKE: u32 = 1 << 2;
const DL_PITSPEED: u32 = 1 << 3;
const DL_TC: u32 = 1 << 4;
const DL_SIGNAL_L: u32 = 1 << 5;
const DL_SIGNAL_R: u32 = 1 << 6;
const DL_OILWARN: u32 = 1 << 8;
const DL_BATTERY: u32 = 1 << 9;
const DL_ABS: u32 = 1 << 10;

#[repr(C)]
#[derive(Clone, Copy)]
struct OutGaugePacket {
    time: u32,    // 0 — milliseconds since start (BeamNG hardcodes 0)
    car: [u8; 4], // "beam" from BeamNG
    flags: u16,   // OG_x bitmask
    gear: i8,     // 0=R, 1=N, 2=1st, ...
    plid: i8,     // unused (BeamNG hardcodes 0)
    speed: f32,   // m/s
    rpm: f32,
    turbo: f32,        // bar
    eng_temp: f32,     // °C
    fuel: f32,         // 0..=1
    oil_pressure: f32, // bar (BeamNG hardcodes 0)
    oil_temp: f32,     // °C
    dash_lights: u32,  // DL_x mask of lights this car has
    show_lights: u32,  // DL_x mask of lights currently lit
    throttle: f32,     // 0..=1
    brake: f32,        // 0..=1
    clutch: f32,       // 0..=1
    display1: [u8; 16],
    display2: [u8; 16],
    id: i32,
}

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
        log_packet(&buf[..n]);
    }
}

fn log_packet(buf: &[u8]) {
    if buf.len() < MIN_PACKET_BYTES {
        info!(
            "short packet ({} bytes) — expected >= {MIN_PACKET_BYTES}",
            buf.len()
        );
        return;
    }
    // Pad to FULL_PACKET_BYTES so the optional `id` field is read as 0
    // for LFS-style 92-byte packets.
    let mut padded = [0u8; FULL_PACKET_BYTES];
    let copy_len = buf.len().min(FULL_PACKET_BYTES);
    padded[..copy_len].copy_from_slice(&buf[..copy_len]);
    // SAFETY: OutGaugePacket is repr(C), all fields are POD, and padded
    // has at least size_of::<OutGaugePacket>() bytes.
    let p: OutGaugePacket = unsafe { ptr::read_unaligned(padded.as_ptr().cast()) };

    let car = car_name(&p.car);
    let gear = gear_label(p.gear);
    let speed_kmh = p.speed * 3.6;
    let mut lights: Vec<&str> = Vec::new();
    let on = p.show_lights;
    if on & DL_SHIFT != 0 {
        lights.push("SHIFT");
    }
    if on & DL_HANDBRAKE != 0 {
        lights.push("HBRAKE");
    }
    if on & DL_PITSPEED != 0 {
        lights.push("PIT");
    }
    if on & DL_OILWARN != 0 {
        lights.push("OIL");
    }
    if on & DL_BATTERY != 0 {
        lights.push("BAT");
    }
    if on & DL_ABS != 0 {
        lights.push("ABS");
    }
    if on & DL_TC != 0 {
        lights.push("TC");
    }
    if on & DL_FULLBEAM != 0 {
        lights.push("HIGHBEAM");
    }
    if on & DL_SIGNAL_L != 0 {
        lights.push("←");
    }
    if on & DL_SIGNAL_R != 0 {
        lights.push("→");
    }
    let lights_str = if lights.is_empty() {
        String::new()
    } else {
        format!(" [{}]", lights.join(" "))
    };
    let turbo = if p.flags & OG_TURBO != 0 {
        format!(" turbo={:.2}bar", p.turbo)
    } else {
        String::new()
    };

    info!(
        "car={car} rpm={:>5.0} gear={gear} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h fuel={:>3.0}% engT={:>3.0}°C{turbo}{lights_str}",
        p.rpm,
        p.throttle * 100.0,
        p.brake * 100.0,
        p.clutch * 100.0,
        speed_kmh,
        p.fuel * 100.0,
        p.eng_temp,
    );
}

fn car_name(buf: &[u8; 4]) -> String {
    let trimmed: Vec<u8> = buf.iter().copied().take_while(|&b| b != 0).collect();
    String::from_utf8(trimmed).unwrap_or_else(|_| "?".to_string())
}

fn gear_label(gear: i8) -> String {
    // BeamNG / LFS convention: 0 = R, 1 = N, 2 = 1st, 3 = 2nd, ...
    match gear {
        0 => "R".to_string(),
        1 => "N".to_string(),
        n if n > 1 => (n - 1).to_string(),
        n => format!("{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_size_matches_outgauge_with_id() {
        assert_eq!(FULL_PACKET_BYTES, 96);
    }

    #[test]
    fn gear_labels_use_lfs_convention() {
        assert_eq!(gear_label(0), "R");
        assert_eq!(gear_label(1), "N");
        assert_eq!(gear_label(2), "1");
        assert_eq!(gear_label(7), "6");
    }

    #[test]
    fn car_name_strips_trailing_nulls() {
        assert_eq!(car_name(&[b'b', b'e', b'a', b'm']), "beam");
        assert_eq!(car_name(&[b'X', b'Y', 0, 0]), "XY");
        assert_eq!(car_name(&[0, 0, 0, 0]), "");
    }
}
