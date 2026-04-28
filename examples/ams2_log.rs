// Listen for Automobilista 2 / Project CARS 2 UDP telemetry and log a
// one-line summary per Telemetry Data packet (packet type 0).
//
// AMS2 is built on Slightly Mad Studios' Madness engine and reuses the
// Project CARS 2 UDP format wholesale. Default port 5606. Multiple packet
// types share that port — every datagram starts with a 12-byte header
// whose `packet_type` byte (offset 10) tells you which struct follows.
//
// Schema source: MacManley/project-cars-2-udp (C++ headers translated
// field-for-field). All structs use `#pragma pack(1)`, mirrored here as
// `#[repr(C, packed)]`.
//
// Enable in AMS2:
//   Options → System → Shared Memory: Project CARS 2,
//                       UDP Protocol Version: Project CARS 2,
//                       UDP Frequency: 1+
//
// Run:
//   cargo run --example ams2_log
//   cargo run --example ams2_log -- --port 5606

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::ptr;

use log::{error, info};

const DEFAULT_PORT: u16 = 5606;
const PACKET_TYPE_TELEMETRY: u8 = 0;
const HEADER_BYTES: usize = std::mem::size_of::<Header>();
const TELEMETRY_PACKET_BYTES: usize = std::mem::size_of::<TelemetryPacket>();

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Header {
    packet_number: u32,
    category_packet_number: u32,
    partial_packet_index: u8,
    partial_packet_number: u8,
    packet_type: u8,
    packet_version: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TelemetryData {
    viewed_participant_index: i8,
    unfiltered_throttle: u8,
    unfiltered_brake: u8,
    unfiltered_steering: i8,
    unfiltered_clutch: u8,
    car_flags: u8,
    oil_temp_celsius: i16,
    oil_pressure_kpa: u16,
    water_temp_celsius: i16,
    water_pressure_kpa: i16,
    fuel_pressure_kpa: u16,
    fuel_capacity: u8,
    brake: u8,
    throttle: u8,
    clutch: u8,
    fuel_level: f32,
    speed: f32,
    rpm: u16,
    max_rpm: u16,
    steering: i8,
    /// Low nibble: gear (0=N, 1..n=forward, 15=R). High nibble: total gears.
    gear_num_gears: u8,
    boost_amount: u8,
    crash_state: u8,
    odometer_km: f32,
    orientation: [f32; 3],
    local_velocity: [f32; 3],
    world_velocity: [f32; 3],
    angular_velocity: [f32; 3],
    local_acceleration: [f32; 3],
    world_acceleration: [f32; 3],
    extents_centre: [f32; 3],
    tyre_flags: [u8; 4],
    terrains: [u8; 4],
    tyre_y: [f32; 4],
    tyre_rps: [f32; 4],
    tyre_temp: [u8; 4],
    tyre_height_above_ground: [f32; 4],
    tyre_wear: [u8; 4],
    brake_damage: [u8; 4],
    suspension_damage: [u8; 4],
    brake_temp_celsius: [i16; 4],
    tyre_tread_temp: [u16; 4],
    tyre_layer_temp: [u16; 4],
    tyre_carcass_temp: [u16; 4],
    tyre_rim_temp: [u16; 4],
    tyre_internal_air_temp: [u16; 4],
    tyre_temp_left: [u16; 4],
    tyre_temp_center: [u16; 4],
    tyre_temp_right: [u16; 4],
    wheel_local_position_y: [f32; 4],
    ride_height: [f32; 4],
    suspension_travel: [f32; 4],
    suspension_velocity: [f32; 4],
    suspension_ride_height: [u16; 4],
    air_pressure: [u16; 4],
    engine_speed: f32,
    engine_torque: f32,
    wings: [u8; 2],
    hand_brake: u8,
    aero_damage: u8,
    engine_damage: u8,
    joy_pad: u32,
    d_pad: u8,
    tyre_compound: [[u8; 40]; 4],
    turbo_boost_pressure: f32,
    full_position: [f32; 3],
    brake_bias: u8,
    tick_count: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TelemetryPacket {
    header: Header,
    data: TelemetryData,
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
                eprintln!("usage: ams2_log [--port PORT]");
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
    info!("listening for AMS2/PC2 telemetry on udp://{bind_addr}");
    info!("(Telemetry Data is packet type 0; other packet types will be skipped)");

    let mut buf = vec![0u8; 2048];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        handle_packet(&buf[..n]);
    }
}

fn handle_packet(buf: &[u8]) {
    if buf.len() < HEADER_BYTES {
        return;
    }
    // SAFETY: Header is repr(C, packed) and buf has at least HEADER_BYTES.
    let header: Header = unsafe { ptr::read_unaligned(buf.as_ptr().cast()) };
    let pkt_type = header.packet_type;

    if pkt_type != PACKET_TYPE_TELEMETRY {
        log::debug!(
            "skipping packet type {pkt_type} ({} bytes) — not telemetry",
            buf.len()
        );
        return;
    }

    if buf.len() < TELEMETRY_PACKET_BYTES {
        info!(
            "short telemetry packet: {} bytes, expected >= {TELEMETRY_PACKET_BYTES}",
            buf.len()
        );
        return;
    }
    // SAFETY: TelemetryPacket is repr(C, packed) and buf has at least
    // TELEMETRY_PACKET_BYTES.
    let pkt: TelemetryPacket = unsafe { ptr::read_unaligned(buf.as_ptr().cast()) };
    log_telemetry(&pkt.data);
}

fn log_telemetry(t: &TelemetryData) {
    // packed-struct field reads must be copied out before formatting.
    let rpm = { t.rpm };
    let max_rpm = { t.max_rpm };
    let speed_kmh = { t.speed } * 3.6;
    let throttle_pct = { t.throttle } as f32 * 100.0 / 255.0;
    let brake_pct = { t.brake } as f32 * 100.0 / 255.0;
    let clutch_pct = { t.clutch } as f32 * 100.0 / 255.0;
    let gear_byte = t.gear_num_gears;
    let gear = gear_byte & 0x0F;
    let num_gears = (gear_byte >> 4) & 0x0F;
    let gear_label = match gear {
        0 => "N".to_string(),
        15 => "R".to_string(),
        n => format!("{n}"),
    };
    let fuel_pct = { t.fuel_level } * 100.0;
    let oil_temp = { t.oil_temp_celsius };
    let water_temp = { t.water_temp_celsius };

    info!(
        "rpm={rpm:>5}/{max_rpm:<5} gear={gear_label}/{num_gears} thr={throttle_pct:>3.0}% \
         brk={brake_pct:>3.0}% clu={clutch_pct:>3.0}% speed={speed_kmh:>5.1}km/h \
         fuel={fuel_pct:>3.0}% oilT={oil_temp}°C waterT={water_temp}°C"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_size_matches_pcars2_pbase() {
        assert_eq!(HEADER_BYTES, 12);
    }

    #[test]
    fn telemetry_packet_size_matches_pcars2_layout() {
        // Cross-checked against MacManley/project-cars-2-udp:
        //   header (12) + TelemetryData (547) = 559.
        // If this fails, AMS2/PC2 has either changed the wire layout or
        // we've made a transcription error.
        assert_eq!(TELEMETRY_PACKET_BYTES, 559);
    }
}
