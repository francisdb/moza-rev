// Listen for Forza "Data Out" UDP telemetry and log a one-line summary
// per packet. Works for FM7 / FH4 / FH5 (Sled and Dash variants) — we
// only decode the first 20 bytes (race-on flag + RPM / max RPM / idle
// RPM), which are stable across every Forza variant.
//
// Setup (in-game, no file edits):
//   Settings → HUD and Gameplay → Data Out: On
//   Data Out IP Address: 127.0.0.1
//   Data Out IP Port:    9999  (or pass --port)
//   Data Out Packet Format: Car Dash  (Sled also works for this example)
//
// Run:
//   cargo run --example forza_log
//   cargo run --example forza_log -- --port 5300
//   cargo run --example forza_log -- --raw          # log every packet
//
// The game sends ~60 Hz; default output is throttled to ~10 Hz so the
// terminal stays readable.

use std::env;
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use log::{error, info, warn};

use moza_rev::forza::{DEFAULT_PORT, Header, MIN_PACKET_BYTES};

const LOG_INTERVAL: Duration = Duration::from_millis(100);
const STREAM_POLL: Duration = Duration::from_millis(200);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut port: u16 = DEFAULT_PORT;
    let mut raw = false;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                let v = it.next().unwrap_or_default();
                port = match v.parse() {
                    Ok(n) => n,
                    Err(e) => {
                        error!("invalid --port {v}: {e}");
                        return ExitCode::from(2);
                    }
                };
            }
            "--raw" => raw = true,
            "--help" | "-h" => {
                eprintln!(
                    "usage: forza_log [--port PORT] [--raw]\n\
                     \n\
                       --port  UDP port to listen on (default {DEFAULT_PORT}; must match\n\
                               in-game Data Out IP Port)\n\
                       --raw   log every packet — by default output is throttled to ~10 Hz",
                );
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
    if let Err(e) = socket.set_read_timeout(Some(STREAM_POLL)) {
        warn!("set read timeout: {e}");
    }
    info!(
        "listening for Forza Data Out on udp://{bind_addr} \
         (set the same port in `Settings → HUD and Gameplay → Data Out`)"
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = shutdown.clone();
        if let Err(e) = ctrlc::set_handler(move || s.store(true, Ordering::SeqCst)) {
            warn!("install signal handler: {e}");
        }
    }

    let mut buf = vec![0u8; 1024];
    let mut last_log = Instant::now()
        .checked_sub(LOG_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut packets_since_log: u32 = 0;

    while !shutdown.load(Ordering::SeqCst) {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(h) = Header::from_bytes(&buf[..n]) else {
            warn!(
                "short packet: {n} bytes, expected ≥ {MIN_PACKET_BYTES} \
                 (something else sending to this port?)"
            );
            continue;
        };
        packets_since_log += 1;
        if !raw && last_log.elapsed() < LOG_INTERVAL {
            continue;
        }
        log_packet(&h, packets_since_log, raw, n);
        last_log = Instant::now();
        packets_since_log = 0;
    }

    info!("shutting down");
    ExitCode::SUCCESS
}

fn log_packet(h: &Header, packets_in_window: u32, raw: bool, total_len: usize) {
    let race = if h.is_race_on() { "RACE" } else { "menu" };
    let rate_tag = if raw {
        String::new()
    } else {
        format!(" ({packets_in_window} pkt)")
    };
    let kind = match total_len {
        232 => "Sled",
        311 => "Dash/FM7",
        324 => "Dash/FH",
        _ => "Dash?",
    };
    info!(
        "[{race}] {kind} {total_len}B  rpm={:>5} / max={:>5} idle={:>4}{rate_tag}",
        h.rpm(),
        h.redline_rpm(),
        h.idle_rpm(),
    );
}
