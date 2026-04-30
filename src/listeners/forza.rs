// Forza "Data Out" listener - works for Forza Motorsport 7, Forza
// Horizon 4, and Forza Horizon 5. Only the shared 20-byte header is
// decoded here (`is_race_on` + RPM / max RPM / idle RPM); that prefix
// is stable across every Forza variant. Unlike BeamNG and AC1, Forza
// transmits the redline directly so no adaptive tracking is needed.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::forza;
use crate::listeners::{EngineState, GameId, Update};

const GAME: GameId = GameId::Forza;

pub fn spawn(port: u16, tx: mpsc::Sender<Update>) -> bool {
    let bind_addr = format!("0.0.0.0:{port}");
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind {bind_addr}: {e}");
            return false;
        }
    };
    info!(
        "listening for {} telemetry on udp://{bind_addr}",
        GAME.name()
    );
    thread::Builder::new()
        .name(format!("listener-{}", GAME.label()))
        .spawn(move || run(socket, tx))
        .expect("failed to spawn listener thread");
    true
}

fn run(socket: UdpSocket, tx: mpsc::Sender<Update>) {
    let mut buf = vec![0u8; 1024];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(engine) = parse(&buf[..n]) else {
            continue;
        };
        if tx.send(Update { game: GAME, engine }).is_err() {
            return;
        }
    }
}

fn parse(buf: &[u8]) -> Option<EngineState> {
    let h = forza::Header::from_bytes(buf)?;
    // Skip menu / paused frames where the engine isn't running.
    if !h.is_race_on() {
        return None;
    }
    let redline = h.redline_rpm();
    let idle = h.idle_rpm();
    // Defensive guard: if the game hasn't initialised the engine fully
    // yet, redline can be 0.
    if redline <= idle.max(1) {
        return None;
    }
    Some(EngineState {
        rpm: h.rpm(),
        rpm_redline: redline,
        rpm_idle: idle,
    })
}
