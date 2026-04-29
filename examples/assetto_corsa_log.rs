// Listen for Assetto Corsa 1 telemetry and log a one-line summary per
// RTCarInfo packet.
//
// AC1 is request-response: we open a UDP socket, send a Handshake to AC's
// listener (default 9996), receive a HandshakeResponse with car/track,
// then send Subscribe-Update to start the per-physics-step stream
// (~333 Hz). On Ctrl-C we send Dismiss to unsubscribe cleanly.
//
// AC needs no configuration to enable this — port 9996 is always
// listening once a session is loaded. If the handshake times out,
// either no session is active yet or AC is bound to a different
// address (e.g. inside Proton's network namespace) — in that case
// pass `--target 127.0.0.1:9996` explicitly.
//
// Run:
//   cargo run --example assetto_corsa_log
//   cargo run --example assetto_corsa_log -- --target 127.0.0.1:9996
//   cargo run --example assetto_corsa_log -- --spot   # 1 Hz instead of 333 Hz
//   cargo run --example assetto_corsa_log -- --raw    # log every packet (no throttle)

use std::env;
use std::net::{SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use log::{error, info, warn};

use moza_rev::assetto_corsa::{
    DEFAULT_PORT, HANDSHAKE_RESPONSE_BYTES, Handshake, HandshakeResponse, RT_CAR_INFO_BYTES,
    RtCarInfo, op,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// AC's UPDATE subscription fires ~333 Hz; that's unreadable in a
/// terminal. Default to a friendly ~10 Hz log rate; `--raw` disables.
const LOG_INTERVAL: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut target: SocketAddr = format!("127.0.0.1:{DEFAULT_PORT}").parse().unwrap();
    let mut subscribe_op = op::SUBSCRIBE_UPDATE;
    let mut raw = false;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--target" | "-t" => {
                let v = it.next().unwrap_or_default();
                target = match v.parse() {
                    Ok(a) => a,
                    Err(e) => {
                        error!("invalid --target {v}: {e}");
                        return ExitCode::from(2);
                    }
                };
            }
            "--spot" => subscribe_op = op::SUBSCRIBE_SPOT,
            "--update" => subscribe_op = op::SUBSCRIBE_UPDATE,
            "--raw" => raw = true,
            "--help" | "-h" => {
                eprintln!(
                    "usage: assetto_corsa_log [--target IP:PORT] [--spot|--update] [--raw]\n\
                     \n\
                       --target  AC's listener address (default 127.0.0.1:9996)\n\
                       --update  subscribe to per-physics-step packets (~333 Hz, default)\n\
                       --spot    subscribe to once-per-second packets\n\
                       --raw     log every packet — by default UPDATE output is\n\
                                 throttled to ~10 Hz so the terminal stays readable",
                );
                return ExitCode::SUCCESS;
            }
            other => {
                error!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            error!("bind ephemeral socket: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = socket.connect(target) {
        error!("connect to {target}: {e}");
        return ExitCode::FAILURE;
    }
    info!(
        "connecting to AC at udp://{target} from {}",
        socket.local_addr().map(|a| a.to_string()).unwrap_or_default()
    );

    // Handshake.
    if let Err(e) = socket.send(&Handshake::new(op::HANDSHAKE).to_bytes()) {
        error!("send handshake: {e}");
        return ExitCode::FAILURE;
    }

    if let Err(e) = socket.set_read_timeout(Some(HANDSHAKE_TIMEOUT)) {
        error!("set read timeout: {e}");
        return ExitCode::FAILURE;
    }

    let mut buf = vec![0u8; 1024];
    let n = match socket.recv(&mut buf) {
        Ok(n) => n,
        Err(e) => {
            error!(
                "no handshake reply within {}s ({e}). Is a session loaded in AC?",
                HANDSHAKE_TIMEOUT.as_secs()
            );
            return ExitCode::FAILURE;
        }
    };
    let Some(hs) = HandshakeResponse::from_bytes(&buf[..n]) else {
        error!(
            "handshake reply too short: got {n} bytes, expected {HANDSHAKE_RESPONSE_BYTES}"
        );
        return ExitCode::FAILURE;
    };
    info!(
        "handshake ok: car={:?} track={:?}{} driver={:?}",
        hs.car(),
        hs.track(),
        {
            let cfg = hs.track_config_name();
            if cfg.is_empty() {
                String::new()
            } else {
                format!(" ({cfg})")
            }
        },
        hs.driver(),
    );

    // Subscribe — server starts streaming RTCarInfo immediately.
    if let Err(e) = socket.send(&Handshake::new(subscribe_op).to_bytes()) {
        error!("send subscribe: {e}");
        return ExitCode::FAILURE;
    }
    info!(
        "subscribed ({}); listening for RTCarInfo packets",
        match subscribe_op {
            op::SUBSCRIBE_UPDATE => "UPDATE ~333 Hz",
            op::SUBSCRIBE_SPOT => "SPOT ~1 Hz",
            _ => "?",
        }
    );

    // No timeout for the streaming phase — rely on user Ctrl-C.
    if let Err(e) = socket.set_read_timeout(None) {
        warn!("clear read timeout: {e}");
    }

    let mut last_log = Instant::now()
        .checked_sub(LOG_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut packets_since_log: u32 = 0;
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        packets_since_log += 1;
        if !raw && last_log.elapsed() < LOG_INTERVAL {
            continue;
        }
        log_packet(&buf[..n], packets_since_log, raw);
        last_log = Instant::now();
        packets_since_log = 0;
    }
}

fn log_packet(buf: &[u8], packets_in_window: u32, raw: bool) {
    if buf.len() < RT_CAR_INFO_BYTES {
        warn!(
            "short packet: {} bytes, expected {RT_CAR_INFO_BYTES}",
            buf.len()
        );
        return;
    }
    let Some(p) = RtCarInfo::from_bytes(buf) else {
        return;
    };
    let reported_size = { p.size };
    if reported_size as usize != RT_CAR_INFO_BYTES {
        warn!(
            "packet self-reported size {reported_size} != expected {RT_CAR_INFO_BYTES} \
             (AC protocol drift?)"
        );
    }
    // In throttled mode, surface how many packets were seen since the
    // last log line so the actual stream rate stays visible.
    let rate_tag = if raw {
        String::new()
    } else {
        format!(" ({packets_in_window} pkt)")
    };
    info!(
        "rpm={:>5} gear={} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% \
         speed={:>5.1}km/h lap={} t={:.2}s pos={:.2}{rate_tag}",
        p.rpm(),
        p.gear_label(),
        { p.gas } * 100.0,
        { p.brake } * 100.0,
        { p.clutch } * 100.0,
        { p.speed_kmh },
        { p.lap_count },
        p.lap_time_s(),
        { p.car_position_normalized },
    );
}
