// Listen for Assetto Corsa Competizione "Broadcasting" UDP messages
// and log a one-line summary per session and per focused-car update.
//
// ACC's broadcasting API is connection-oriented: we send a REGISTER
// datagram with the connection password from broadcasting.json, ACC
// replies with REGISTRATION_RESULT and then streams REALTIME_UPDATE +
// REALTIME_CAR_UPDATE at the requested cadence. On SIGINT (Ctrl-C) /
// SIGTERM we send UNREGISTER so ACC drops us from its broadcaster list
// immediately rather than timing us out.
//
// Setup (one-time, ACC must be restarted afterward): edit
//   Documents/Assetto Corsa Competizione/Config/broadcasting.json
// (UTF-16 LE) and set
//   "udpListenerPort": 9000,
//   "connectionPassword": "<some password>"
//
// Run (uses the default "moza" password from `assetto_corsa_competizione::
// DEFAULT_PASSWORD` — same value `configure` writes; pass --password to
// override if you've set a different connectionPassword):
//   cargo run --example assetto_corsa_competizione_log
//   cargo run --example assetto_corsa_competizione_log -- --target 127.0.0.1:9000 --interval-ms 250
//   cargo run --example assetto_corsa_competizione_log -- --all-cars
//   cargo run --example assetto_corsa_competizione_log -- --password something-else
//
// Note: ACC's broadcasting API does NOT carry engine RPM. This example
// is for protocol-debug groundwork only — it can't drive moza-rev's LED
// bar without a separate Windows-side shared-memory bridge.

use std::env;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use log::{error, info, warn};

use moza_rev::assetto_corsa_competizione::{
    DEFAULT_PASSWORD, DEFAULT_PORT, Message, RealtimeCarUpdate, RealtimeUpdate, build_register,
    build_unregister, car_location_name, parse_message, session_phase_name, session_type_name,
};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);
/// Stream-loop recv timeout — short enough that we notice a Ctrl-C
/// quickly even when ACC is silent, long enough not to spin the CPU.
const STREAM_POLL: Duration = Duration::from_millis(200);
const DEFAULT_INTERVAL_MS: i32 = 250;
const DEFAULT_DISPLAY_NAME: &str = "moza-rev-log";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut target: SocketAddr = format!("127.0.0.1:{DEFAULT_PORT}").parse().unwrap();
    let mut password = DEFAULT_PASSWORD.to_string();
    let mut command_password = String::new();
    let mut display_name = DEFAULT_DISPLAY_NAME.to_string();
    let mut interval_ms = DEFAULT_INTERVAL_MS;
    let mut all_cars = false;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--target" | "-t" => target = parse_or_exit(it.next().as_deref(), "--target"),
            "--password" | "-p" => password = it.next().unwrap_or_default(),
            "--command-password" => command_password = it.next().unwrap_or_default(),
            "--display-name" => display_name = it.next().unwrap_or_default(),
            "--interval-ms" => interval_ms = parse_or_exit(it.next().as_deref(), "--interval-ms"),
            "--all-cars" => all_cars = true,
            "--help" | "-h" => {
                eprintln!(
                    "usage: assetto_corsa_competizione_log [options]\n\
                     \n\
                       --target IP:PORT       ACC broadcasting listener (default 127.0.0.1:9000)\n\
                       --password PWD         connectionPassword from broadcasting.json\n\
                                              (default {DEFAULT_PASSWORD:?} — matches what `configure` writes)\n\
                       --command-password PWD optional, only needed for write commands\n\
                       --display-name NAME    shown in ACC's broadcaster list (default {DEFAULT_DISPLAY_NAME})\n\
                       --interval-ms N        update cadence (default {DEFAULT_INTERVAL_MS})\n\
                       --all-cars             log every car update, not just focused\n\
                     \n\
                     ACC needs broadcasting.json set with udpListenerPort and connectionPassword,\n\
                     and ACC restarted, before this example will see any reply."
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
        "connecting to ACC at udp://{target} from {} (display name: {display_name:?}, interval {interval_ms}ms)",
        socket
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default()
    );

    let register = build_register(&display_name, &password, interval_ms, &command_password);
    if let Err(e) = socket.send(&register) {
        error!("send register: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = socket.set_read_timeout(Some(REGISTER_TIMEOUT)) {
        error!("set read timeout: {e}");
        return ExitCode::FAILURE;
    }

    let mut buf = vec![0u8; 4096];
    let connection_id = match wait_for_registration(&socket, &mut buf) {
        Ok(id) => id,
        Err(code) => return code,
    };

    if let Err(e) = socket.set_read_timeout(Some(STREAM_POLL)) {
        warn!("set stream read timeout: {e}");
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = shutdown.clone();
        if let Err(e) = ctrlc::set_handler(move || s.store(true, Ordering::SeqCst)) {
            warn!("install signal handler: {e} (Ctrl-C won't UNREGISTER cleanly)");
        }
    }

    info!("listening for telemetry; Ctrl-C to exit");

    let mut focused_car_index: Option<u16> = None;
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
        let Some(msg) = parse_message(&buf[..n]) else {
            warn!("undecodable packet ({n} bytes, first byte {:#04x})", buf[0]);
            continue;
        };
        match msg {
            Message::RegistrationResult(_) => {
                // Re-registrations shouldn't happen mid-stream; log and ignore.
                warn!("unexpected REGISTRATION_RESULT mid-stream");
            }
            Message::RealtimeUpdate(u) => {
                focused_car_index = Some(u.focused_car_index as u16);
                log_session(&u);
            }
            Message::RealtimeCarUpdate(c) => {
                if all_cars || Some(c.car_index) == focused_car_index {
                    log_car(&c, focused_car_index == Some(c.car_index));
                }
            }
            Message::Other { msg_type, body_len } => {
                info!("[type {msg_type}] {body_len} bytes (not decoded)");
            }
        }
    }

    info!("shutting down: sending UNREGISTER (connection_id={connection_id})");
    if let Err(e) = socket.send(&build_unregister(connection_id)) {
        warn!("send unregister: {e}");
    }
    ExitCode::SUCCESS
}

fn wait_for_registration(socket: &UdpSocket, buf: &mut [u8]) -> Result<i32, ExitCode> {
    let n = match socket.recv(buf) {
        Ok(n) => n,
        Err(e) => {
            error!(
                "no REGISTRATION_RESULT within {}s ({e}). Is ACC running with broadcasting.json \
                 configured (udpListenerPort + connectionPassword) and ACC restarted?",
                REGISTER_TIMEOUT.as_secs()
            );
            return Err(ExitCode::FAILURE);
        }
    };
    let Some(msg) = parse_message(&buf[..n]) else {
        error!("undecodable registration reply ({n} bytes)");
        return Err(ExitCode::FAILURE);
    };
    match msg {
        Message::RegistrationResult(r) if r.success => {
            info!(
                "registered: connection_id={} readonly={} (empty errmsg={:?})",
                r.connection_id, r.readonly, r.error_message
            );
            Ok(r.connection_id)
        }
        Message::RegistrationResult(r) => {
            error!(
                "ACC rejected registration: connection_id={} err={:?}",
                r.connection_id, r.error_message
            );
            Err(ExitCode::FAILURE)
        }
        other => {
            error!("expected REGISTRATION_RESULT, got {other:?}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn log_session(u: &RealtimeUpdate) {
    let (h, m) = u.time_of_day();
    info!(
        "[SES] {} / {} focused=#{} t={:.1}s end={:.1}s tod={:02}:{:02} ambient={}°C track={}°C \
         clouds={:.1} rain={:.1} wet={:.1} cam={}/{} hud={} best={}",
        session_type_name(u.session_type),
        session_phase_name(u.phase),
        u.focused_car_index,
        u.session_time_ms / 1000.0,
        u.session_end_time_ms / 1000.0,
        h,
        m,
        u.ambient_temp_c,
        u.track_temp_c,
        u.clouds,
        u.rain_level,
        u.wetness,
        u.active_camera_set,
        u.active_camera,
        u.current_hud_page,
        u.best_session_lap.format(),
    );
}

fn log_car(c: &RealtimeCarUpdate, focused: bool) {
    let marker = if focused { "*" } else { " " };
    info!(
        "[CAR{marker}#{:>2}] gear={} speed={:>3}km/h pos={}/{} loc={} spline={:.3} lap={} \
         current={} last={} best={} delta={}ms",
        c.car_index,
        c.gear_label(),
        c.kmh,
        c.position,
        c.track_position,
        car_location_name(c.car_location),
        c.spline_position,
        c.laps,
        c.current_lap.format(),
        c.last_lap.format(),
        c.best_session_lap.format(),
        c.delta_ms,
    );
}

fn parse_or_exit<T>(s: Option<&str>, name: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let v = s.unwrap_or_default();
    match v.parse() {
        Ok(t) => t,
        Err(e) => {
            error!("invalid {name} {v:?}: {e}");
            std::process::exit(2);
        }
    }
}
