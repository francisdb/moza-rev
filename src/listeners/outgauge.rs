// LFS OutGauge listener (BeamNG.drive, Live For Speed, anything else
// that emits the OutGauge UDP packet format).
//
// OutGauge transmits current RPM but not redline or idle, so this
// listener tracks the highest RPM seen across the session as an
// adaptive redline. Idle is fixed at a typical petrol-car value
// (engine-off / car-not-loaded frames are dropped).

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::listeners::{EngineState, GameId, Update};
use crate::outgauge;

const GAME: GameId = GameId::BeamNG;

/// Initial redline guess for a typical petrol car. Replaced by the
/// max observed RPM as soon as the driver revs above it.
const INITIAL_REDLINE: f32 = 7000.0;

/// Engine-off threshold and idle floor for the LED math. Most petrol
/// engines idle around 700-900 RPM; below this we treat the engine as
/// off and skip the frame.
const ASSUMED_IDLE: i32 = 800;

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
    let mut max_rpm: f32 = INITIAL_REDLINE;
    let mut buf = vec![0u8; 256];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("recv: {e}");
                continue;
            }
        };
        let Some(packet) = outgauge::Packet::from_bytes(&buf[..n]) else {
            continue;
        };
        let rpm_f = { packet.rpm };
        if rpm_f <= 1.0 {
            continue; // engine off / car not loaded
        }
        if rpm_f > max_rpm {
            max_rpm = rpm_f;
        }
        let engine = EngineState {
            rpm: rpm_f as i32,
            rpm_redline: max_rpm as i32,
            rpm_idle: ASSUMED_IDLE,
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
