// Listen for DiRT Rally 2.0 telemetry and log a one-line summary per packet.
// Uses the shared `codemasters_legacy` parser, so the same example also
// works for any other Codemasters EGO-engine title (DiRT 2/3/Showdown,
// F1 2010-2017, GRID series, DiRT Rally) with telemetry enabled.
//
// Enable telemetry by editing
// `Documents/My Games/DiRT Rally 2.0/hardwaresettings/hardware_settings_config.xml`
// — find the `<motion_platform>` block and set the `<udp>` child to:
//   <udp enabled="true" extradata="3" ip="127.0.0.1" port="20777" delay="1" />
//
// Run:
//   cargo run --example dr2_log
//   cargo run --example dr2_log -- --port 20777

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;

use log::{error, info};

use moza_rev::codemasters_legacy::{DEFAULT_PORT, PACKET_BYTES, Telemetry};

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
    if buf.len() < PACKET_BYTES {
        info!(
            "short packet ({} bytes) — set extradata=3 for the full {PACKET_BYTES}-byte packet",
            buf.len()
        );
        // Still log what we can, with trailing fields zeroed.
        log_telemetry(&Telemetry::from_bytes_partial(buf));
        return;
    }
    if let Some(t) = Telemetry::from_bytes(buf) {
        log_telemetry(&t);
    }
}

fn log_telemetry(t: &Telemetry) {
    info!(
        "rpm={}/{} (idle {}) gear={} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h pos={} lap={}/{} laptime={:.2}s",
        t.rpm(),
        t.redline_rpm(),
        t.idle_rpm(),
        t.gear_label(),
        { t.throttle * 100.0 },
        { t.brake * 100.0 },
        { t.clutch * 100.0 },
        t.speed_kmh(),
        { t.car_pos as i32 },
        { t.current_lap as i32 },
        { t.total_laps as i32 },
        { t.lap_time },
    );
}
