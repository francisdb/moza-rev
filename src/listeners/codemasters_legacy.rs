// Codemasters EGO "extradata" UDP listener (DR2, DR1, DiRT 2/3,
// DiRT Showdown, F1 2010-2017, GRID series - they all share one wire
// format on a single port).

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::codemasters_legacy::Telemetry;
use crate::listeners::{EngineState, GameId, Update};

const GAME: GameId = GameId::CodemastersLegacy;

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
    let mut buf = vec![0u8; 2048];
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
    let t = Telemetry::from_bytes(buf)?;
    // Skip frames before the engine is "real" (in menus, redline=0).
    if t.redline_rpm() <= t.idle_rpm() {
        return None;
    }
    Some(EngineState {
        rpm: t.rpm(),
        rpm_redline: t.redline_rpm(),
        rpm_idle: t.idle_rpm(),
    })
}
