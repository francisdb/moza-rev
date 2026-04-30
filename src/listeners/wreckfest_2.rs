// Wreckfest 2 ("Pino") UDP telemetry listener.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::listeners::{EngineState, GameId, Update};
use crate::wreckfest;

const GAME: GameId = GameId::Wreckfest2;

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
        if tx
            .send(Update {
                game: GAME,
                engine,
            })
            .is_err()
        {
            return;
        }
    }
}

fn parse(buf: &[u8]) -> Option<EngineState> {
    let main = match wreckfest::parse(buf)? {
        wreckfest::Packet::Main(m) => m,
        _ => return None,
    };
    let engine = main.car.engine;
    Some(EngineState {
        rpm: { engine.rpm },
        rpm_redline: { engine.rpm_redline },
        rpm_idle: { engine.rpm_idle },
    })
}
