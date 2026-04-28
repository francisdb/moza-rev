// Listen for DiRT Rally 2.0 telemetry and log a one-line summary per packet.
//
// DR2's UDP format is the Codemasters "extradata" protocol: a contiguous
// little-endian f32 array, no header, no signature. With `extradata=3` the
// packet is 264 bytes (66 fields). RPM/max-RPM/idle-RPM are encoded as
// `actual_rpm / 10` per Codemasters convention.
//
// Schema cross-referenced from
// https://github.com/ErlerPhilipp/dr2_logger/blob/master/source/dirt_rally/udp_data.py
//
// Enable telemetry by editing
// `Documents/My Games/DiRT Rally 2.0/hardwaresettings/hardware_settings_config.xml`
// and setting:
//   <motion enabled="true" ip="127.0.0.1" port="20777" extradata="3" delay="1"/>
//
// Run:
//   cargo run --example dr2_log
//   cargo run --example dr2_log -- --port 20777

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;

use log::{error, info};

const DEFAULT_PORT: u16 = 20777;
const MIN_BYTES_FOR_RPM: usize = (FIELD_RPM + 1) * 4;

// Field indices into the f32 array (multiply by 4 for byte offset).
const FIELD_RUN_TIME: usize = 0;
const FIELD_LAP_TIME: usize = 1;
const FIELD_DISTANCE: usize = 2;
const FIELD_SPEED_MS: usize = 7;
const FIELD_THROTTLE: usize = 29;
const FIELD_STEERING: usize = 30;
const FIELD_BRAKE: usize = 31;
const FIELD_CLUTCH: usize = 32;
const FIELD_GEAR: usize = 33;
const FIELD_CURRENT_LAP: usize = 36;
const FIELD_RPM: usize = 37; // value × 10 = actual RPM
const FIELD_CAR_POS: usize = 39;
const FIELD_LAPS_COMPLETED: usize = 59;
const FIELD_TOTAL_LAPS: usize = 60;
const FIELD_MAX_RPM: usize = 63; // value × 10
const FIELD_IDLE_RPM: usize = 64; // value × 10

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
                eprintln!("usage: dr2_log [--port PORT]");
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
    info!("listening for DiRT Rally 2.0 telemetry on udp://{bind_addr}");
    info!("(set extradata=3 in hardware_settings_config.xml for full data)");

    let mut buf = vec![0u8; 1024];
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
    if buf.len() < MIN_BYTES_FOR_RPM {
        info!(
            "short packet ({} bytes) — enable extradata=3 for full data",
            buf.len()
        );
        return;
    }

    let rpm = (read_f32(buf, FIELD_RPM) * 10.0) as i32;
    let max_rpm = (read_f32(buf, FIELD_MAX_RPM) * 10.0) as i32;
    let idle_rpm = (read_f32(buf, FIELD_IDLE_RPM) * 10.0) as i32;
    let gear = read_f32(buf, FIELD_GEAR) as i32;
    let throttle = read_f32(buf, FIELD_THROTTLE);
    let brake = read_f32(buf, FIELD_BRAKE);
    let clutch = read_f32(buf, FIELD_CLUTCH);
    let speed_kmh = read_f32(buf, FIELD_SPEED_MS) * 3.6;
    let lap = read_f32(buf, FIELD_CURRENT_LAP) as i32;
    let total_laps = read_f32(buf, FIELD_TOTAL_LAPS) as i32;
    let pos = read_f32(buf, FIELD_CAR_POS) as i32;
    let lap_time = read_f32(buf, FIELD_LAP_TIME);
    let _ = (
        read_f32(buf, FIELD_RUN_TIME),
        read_f32(buf, FIELD_DISTANCE),
        read_f32(buf, FIELD_STEERING),
        read_f32(buf, FIELD_LAPS_COMPLETED),
    );

    info!(
        "rpm={rpm}/{max_rpm} (idle {idle_rpm}) gear={} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h pos={pos} lap={lap}/{total_laps} laptime={:.2}s",
        gear_label(gear),
        throttle * 100.0,
        brake * 100.0,
        clutch * 100.0,
        speed_kmh,
        lap_time,
    );
}

fn read_f32(buf: &[u8], field_idx: usize) -> f32 {
    let off = field_idx * 4;
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn gear_label(gear: i32) -> String {
    // Codemasters convention: 0 = N/R, gear values go 0..=max_gears.
    match gear {
        0 => "N".to_string(),
        n if n < 0 => "R".to_string(),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_f32_decodes_le_bytes() {
        // Field 37 (RPM): write 700.0 (= 7000 RPM after ×10)
        let mut buf = vec![0u8; 264];
        let off = FIELD_RPM * 4;
        buf[off..off + 4].copy_from_slice(&700.0f32.to_le_bytes());
        assert_eq!(read_f32(&buf, FIELD_RPM), 700.0);
    }
}
