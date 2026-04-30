// Assetto Corsa 1 (RTCarInfo) listener.
//
// AC is request-response over UDP: this thread opens an ephemeral
// socket, sends a HANDSHAKE to AC's listener port, receives a
// HandshakeResponse with car / track / driver, then sends
// SUBSCRIBE_UPDATE to begin the per-physics-step stream (~333 Hz).
//
// We retry the handshake every `HANDSHAKE_RETRY` so it doesn't matter
// whether AC is started before or after moza-rev: whenever a session
// loads, this loop picks it up. RTCarInfo carries no redline so we
// adapt like the OutGauge listener does.
//
// The log lines emitted here describe the *handshake-layer* lifecycle
// ("connected", "disconnected"). The runtime in `crate::app` emits
// stream-layer lifecycle events ("telemetry stream from X started")
// in parallel; the two layers stay distinct via different module-path
// log targets and different wording.

use std::net::UdpSocket;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};

use crate::assetto_corsa::{Handshake, HandshakeResponse, RtCarInfo, op};
use crate::listeners::{EngineState, GameId, Update};

const GAME: GameId = GameId::AssettoCorsa;

/// AC's RTCarInfo packet has no redline / max-RPM field, so we adapt
/// like the OutGauge listener. Start at a typical petrol value.
const INITIAL_REDLINE: f32 = 7000.0;
const ASSUMED_IDLE: i32 = 800;

/// How often to retry the handshake when AC isn't responding (no
/// session loaded, or AC not running). Low enough that startup order
/// doesn't matter, high enough not to hammer the network.
const HANDSHAKE_RETRY: Duration = Duration::from_secs(3);

/// Time to wait for AC's handshake reply before giving up and retrying.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(800);

/// Time to wait for the next RTCarInfo packet before treating the
/// session as ended and going back to handshake retry.
const STREAM_TIMEOUT: Duration = Duration::from_secs(2);

/// AC's UPDATE subscription fires every physics step (~333 Hz), 3-5x
/// faster than any other game's telemetry cadence. The wheel only
/// needs LED updates at the heartbeat rate (~4 Hz nominal, more on
/// state change), so we cap the forward rate to a sane ~60 Hz to
/// avoid flooding the channel without dropping LED responsiveness.
const FORWARD_INTERVAL: Duration = Duration::from_millis(16);

/// Spawn the listener thread. AC has no port to bind up-front (the
/// socket is the *client* side of a handshake), so unlike the other
/// listeners this one returns no failure value.
pub fn spawn(port: u16, tx: mpsc::Sender<Update>) {
    let target = format!("127.0.0.1:{port}");
    info!(
        "listening for {} telemetry via udp://{target} (handshake retry every {}s)",
        GAME.name(),
        HANDSHAKE_RETRY.as_secs()
    );
    thread::Builder::new()
        .name(format!("listener-{}", GAME.label()))
        .spawn(move || run(target, tx))
        .expect("failed to spawn listener thread");
}

fn run(target: String, tx: mpsc::Sender<Update>) {
    let mut buf = vec![0u8; 1024];
    let mut max_rpm: f32 = INITIAL_REDLINE;

    loop {
        // Fresh socket per cycle so AC sees a new client. The previous
        // session's ephemeral port may already have a queued Dismiss.
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                error!("bind: {e}");
                thread::sleep(HANDSHAKE_RETRY);
                continue;
            }
        };
        if let Err(e) = socket.connect(&target) {
            debug!("connect {target}: {e}");
            thread::sleep(HANDSHAKE_RETRY);
            continue;
        }
        if let Err(e) = socket.set_read_timeout(Some(HANDSHAKE_TIMEOUT)) {
            error!("set_read_timeout: {e}");
            thread::sleep(HANDSHAKE_RETRY);
            continue;
        }

        if let Err(e) = socket.send(&Handshake::new(op::HANDSHAKE).to_bytes()) {
            debug!("send handshake: {e}");
            thread::sleep(HANDSHAKE_RETRY);
            continue;
        }
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                // Most common case: AC not running, or running but no
                // session loaded. Quiet at debug level - info-level
                // chatter every 3s would be noisy.
                debug!("handshake: no reply, retrying");
                thread::sleep(HANDSHAKE_RETRY);
                continue;
            }
        };
        let Some(hs) = HandshakeResponse::from_bytes(&buf[..n]) else {
            warn!("handshake reply too short ({n} bytes); retrying");
            thread::sleep(HANDSHAKE_RETRY);
            continue;
        };
        info!(
            "connected: car={:?} track={:?} driver={:?}",
            hs.car(),
            hs.track(),
            hs.driver()
        );

        if let Err(e) = socket.send(&Handshake::new(op::SUBSCRIBE_UPDATE).to_bytes()) {
            warn!("subscribe: {e}");
            continue;
        }
        if let Err(e) = socket.set_read_timeout(Some(STREAM_TIMEOUT)) {
            warn!("set stream timeout: {e}");
            continue;
        }

        // Stream loop. On timeout we treat it as session ended and go
        // back to handshake retry: covers menu return, quit, etc. We
        // still receive every packet (so the stream-timeout watchdog
        // works) but only forward at ~60 Hz to avoid spamming the
        // channel at AC's full 333 Hz physics rate.
        let mut last_forwarded = Instant::now()
            .checked_sub(FORWARD_INTERVAL)
            .unwrap_or_else(Instant::now);
        loop {
            let n = match socket.recv(&mut buf) {
                Ok(n) => n,
                Err(_) => {
                    info!("disconnected (stream timed out); resuming handshake retry");
                    let _ = socket.send(&Handshake::new(op::DISMISS).to_bytes());
                    break;
                }
            };
            let Some(p) = RtCarInfo::from_bytes(&buf[..n]) else {
                continue;
            };
            let rpm_f = { p.engine_rpm };
            // Skip menu / engine-off frames.
            if rpm_f <= 1.0 {
                continue;
            }
            if rpm_f > max_rpm {
                max_rpm = rpm_f;
            }
            if last_forwarded.elapsed() < FORWARD_INTERVAL {
                continue;
            }
            last_forwarded = Instant::now();
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
}
