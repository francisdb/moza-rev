// Listen for Automobilista 2 / Project CARS 2 UDP telemetry and log a
// one-line summary per Telemetry Data packet (packet type 0). Other packet
// types (race state, participants, timings, …) get a single line each
// rate-limited per type so the log is informative without spamming.
//
// Enable in AMS2:
//   Options → System → Shared Memory: Project CARS 2,
//                       UDP Protocol Version: Project CARS 2,
//                       UDP Frequency: 1+
//
// Linux gotcha: AMS2 broadcasts to 255.255.255.255 and the Linux kernel
// doesn't loop limited-broadcast back to local sockets. Run this once
// before launching:
//   sudo iptables -t nat -I OUTPUT -p udp -d 255.255.255.255 --dport 5606 \
//     -j DNAT --to-destination 127.0.0.1:5606
//
// Run:
//   cargo run --example automobilista_2_log
//   cargo run --example automobilista_2_log -- --port 5606

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use log::{error, info};

use moza_rev::madness::{self, Header, TelemetryData, TelemetryPacket};

/// Per-packet-type rate-limit for non-telemetry packets so the log isn't
/// spammed by the slower-cadence types (race state, participants, timings…).
const NON_TELEMETRY_COOLDOWN: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut port = madness::DEFAULT_PORT;
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
                eprintln!("usage: automobilista_2_log [--port PORT]");
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
    info!(
        "(Telemetry packets log per-packet; other types log at most once every {}s per type)",
        NON_TELEMETRY_COOLDOWN.as_secs()
    );

    let mut buf = vec![0u8; 2048];
    let mut last_log_per_type: [Option<Instant>; 16] = [None; 16];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        handle_packet(&buf[..n], &mut last_log_per_type);
    }
}

fn handle_packet(buf: &[u8], last_log_per_type: &mut [Option<Instant>; 16]) {
    let Some(header) = Header::from_bytes(buf) else {
        return;
    };
    let pkt_type = header.packet_type;

    if pkt_type == madness::PACKET_TYPE_TELEMETRY {
        match TelemetryPacket::from_bytes(buf) {
            Some(pkt) => log_telemetry(&pkt.data),
            None => info!(
                "short telemetry packet: {} bytes, expected >= {}",
                buf.len(),
                madness::TELEMETRY_PACKET_BYTES
            ),
        }
        return;
    }

    // Non-telemetry packet: log at most once per type per cooldown window.
    let idx = (pkt_type as usize).min(last_log_per_type.len() - 1);
    let now = Instant::now();
    let should_log =
        last_log_per_type[idx].is_none_or(|t| now.duration_since(t) >= NON_TELEMETRY_COOLDOWN);
    if should_log {
        info!(
            "packet type {pkt_type} ({}) — {} bytes",
            madness::packet_type_name(pkt_type),
            buf.len()
        );
        last_log_per_type[idx] = Some(now);
    }
}

fn log_telemetry(t: &TelemetryData) {
    info!(
        "rpm={:>5}/{:<5} gear={}/{} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h fuel={:>3.0}% oilT={}°C waterT={}°C",
        t.rpm(),
        t.redline_rpm(),
        t.gear_label(),
        t.num_gears(),
        t.throttle_frac() * 100.0,
        t.brake_frac() * 100.0,
        t.clutch_frac() * 100.0,
        t.speed_kmh(),
        { t.fuel_level } * 100.0,
        { t.oil_temp_celsius },
        { t.water_temp_celsius },
    );
}
