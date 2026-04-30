// Automobilista 2 / Project CARS 2 (Madness-engine PC2 UDP) listener.
//
// AMS2 doesn't transmit an idle RPM, so we substitute a typical petrol
// idle so the LED math has a sensible floor. Note: AMS2 broadcasts to
// 255.255.255.255; on Linux the kernel doesn't loop limited-broadcast
// packets back to local sockets - see the README's iptables NAT note.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;

use log::{error, info};

use crate::listeners::{EngineState, GameId, Update};
use crate::madness;

const GAME: GameId = GameId::Ams2;

/// AMS2 / PC2 don't transmit an idle RPM. Use a typical petrol-car
/// value; the bar will start lighting slightly above this. Tune via
/// the constant if you race diesels or high-revving race cars.
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
    // Madness multiplexes 9 packet types on the same port;
    // TelemetryPacket::from_bytes filters to type 0 (telemetry).
    let pkt = madness::TelemetryPacket::from_bytes(buf)?;
    let redline = pkt.data.redline_rpm();
    // Skip menu / loading frames where the engine isn't initialized.
    if redline <= 0 {
        return None;
    }
    Some(EngineState {
        rpm: pkt.data.rpm(),
        rpm_redline: redline,
        rpm_idle: ASSUMED_IDLE,
    })
}
