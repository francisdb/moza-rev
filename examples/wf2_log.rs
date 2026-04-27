// Listen for Wreckfest 2 telemetry and log a one-line summary per packet.
// Useful for verifying the game is sending data and for spotting which
// packet types arrive at what rate.
//
// Run:
//   cargo run --example wf2_log
//   cargo run --example wf2_log -- --port 23123

use std::env;
use std::net::UdpSocket;
use std::process::ExitCode;

use log::{error, info};

use moza_rev::wreckfest::{self, Packet, ParticipantStatus, SessionStatus};

const DEFAULT_PORT: u16 = 23123;

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
                eprintln!("usage: wf2_log [--port PORT]");
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
    info!("listening for Wreckfest 2 telemetry on udp://{bind_addr}");

    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(packet) = wreckfest::parse(&buf[..n]) else {
            continue;
        };
        log_packet(&packet);
    }
}

fn log_packet(packet: &Packet) {
    match packet {
        Packet::Main(m) => {
            // Copy nested packed structs out so we can read fields freely.
            let engine = m.car.engine;
            let driveline = m.car.driveline;
            let input = m.car.input;
            let leaderboard = m.leaderboard;
            let timing = m.timing;
            let session_status = m.session.status();
            let rpm = engine.rpm;
            let redline = engine.rpm_redline;
            let gear = driveline.gear;
            let speed_ms = driveline.speed;
            let lap = leaderboard.lap_current;
            let lap_progress = timing.lap_progress;
            info!(
                "Main          rpm={rpm}/{redline} gear={} thr={:>3.0}% brk={:>3.0}% clu={:>3.0}% speed={:.1}km/h lap={lap} prog={:.0}% status={}",
                gear_label(gear),
                input.throttle * 100.0,
                input.brake * 100.0,
                input.clutch * 100.0,
                speed_ms * 3.6,
                lap_progress * 100.0,
                session_label(session_status),
            );
        }
        Packet::ParticipantsLeaderboard(p) => {
            let active = p
                .participants
                .iter()
                .filter(|l| !matches!(l.status(), Some(ParticipantStatus::Unused)))
                .count();
            // Find leader (position == 1) for a tiny extra signal.
            let leader = p.participants.iter().find(|l| l.position == 1);
            match leader {
                Some(l) => {
                    let lap = l.lap_current;
                    info!("Leaderboard   active={active}/36 leader_lap={lap}");
                }
                None => info!("Leaderboard   active={active}/36 (no leader yet)"),
            }
        }
        Packet::ParticipantsTiming(_) => {
            info!("Timing        (per-participant lap/sector deltas)");
        }
        Packet::ParticipantsTimingSectors(_) => {
            info!("TimingSectors (per-participant sector splits)");
        }
        Packet::ParticipantsMotion(_) => {
            info!("Motion        (per-participant orientation+velocity)");
        }
        Packet::ParticipantsInfo(p) => {
            let active = p
                .participants
                .iter()
                .filter(|i| i.participant_index != 255)
                .count();
            info!("Info          active={active}/36 (~1Hz)");
        }
        Packet::ParticipantsDamage(_) => {
            info!("Damage        (per-participant bit-packed damage states, ~2Hz)");
        }
    }
}

fn gear_label(gear: u8) -> String {
    // Driveline::gear: 0 = R, 1 = N, 2..= = forward gears (so 2 = 1st).
    match gear {
        0 => "R".to_string(),
        1 => "N".to_string(),
        n => (n - 1).to_string(),
    }
}

fn session_label(status: Option<SessionStatus>) -> &'static str {
    match status {
        Some(SessionStatus::None) => "NONE",
        Some(SessionStatus::PreRace) => "PRE_RACE",
        Some(SessionStatus::Countdown) => "COUNTDOWN",
        Some(SessionStatus::Racing) => "RACING",
        Some(SessionStatus::Abandoned) => "ABANDONED",
        Some(SessionStatus::PostRace) => "POST_RACE",
        None => "?",
    }
}
