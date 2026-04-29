// Runtime: spawn the per-game UDP listeners, route their EngineState
// updates to a single channel, and drive the Moza wheel's LED bar +
// status output from there. Knows nothing about clap — `run` takes a
// fully-parsed `ListenArgs` from `crate::cli`.

use std::io::{IsTerminal, Write};
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};

use moza_rev::assetto_corsa::{Handshake, HandshakeResponse, RtCarInfo, op};
use moza_rev::codemasters_legacy::Telemetry;
use moza_rev::madness;
use moza_rev::moza::{self, BaseTemps, Moza, Protocol};
use moza_rev::outgauge;
use moza_rev::wreckfest::{self, EngineState};

use crate::cli::ListenArgs;

/// AMS2 / PC2 don't transmit an idle RPM. Use a typical petrol-car value;
/// the bar will start lighting slightly above this. Tweak via the constant
/// if you race diesels or high-revving race cars.
const AMS2_ASSUMED_IDLE: i32 = 800;

/// OutGauge doesn't ship max-RPM, so we adaptively track the highest
/// RPM seen in the BeamNG session. Start sane for typical petrol cars
/// (~7000 redline) so the bar lights up reasonably even before the
/// driver has revved high.
const BEAMNG_INITIAL_REDLINE: f32 = 7000.0;
/// And no idle either. Most petrol engines idle at ~700-900 RPM; below
/// this we treat as engine off and skip frames.
const BEAMNG_ASSUMED_IDLE: i32 = 800;

/// Assetto Corsa's RTCarInfo packet has no redline / max-RPM field, so
/// we adapt like BeamNG does. Start at a typical petrol value.
const AC_INITIAL_REDLINE: f32 = 7000.0;
const AC_ASSUMED_IDLE: i32 = 800;
/// How often to retry the handshake when AC isn't responding (no
/// session loaded, or AC not running). Keep low enough that startup
/// order doesn't matter, high enough not to hammer the network.
const AC_HANDSHAKE_RETRY: Duration = Duration::from_secs(3);
/// Time to wait for AC's handshake reply before giving up and retrying.
const AC_HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(800);
/// Time to wait for the next RTCarInfo packet before treating the
/// session as ended and going back to handshake retry.
const AC_STREAM_TIMEOUT: Duration = Duration::from_secs(2);
/// AC's UPDATE subscription fires every physics step (~333 Hz), 3-5x
/// faster than any other game's telemetry cadence. The wheel only
/// needs LED updates at the heartbeat rate (~4 Hz nominal, more on
/// state change), so we cap the forward rate to a sane ~60 Hz to
/// avoid flooding the channel without dropping LED responsiveness.
const AC_FORWARD_INTERVAL: Duration = Duration::from_millis(16);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

/// How often to retry opening the wheelbase after a disconnect. Linux
/// renumbers `/dev/ttyACMx` on replug, so each retry re-runs the
/// autodetection (unless the user pinned `--serial`).
const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

/// Time without a packet from the active game before we revert to "no game"
/// and clear the LED bar.
const IDLE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often to read the wheelbase temperature sensors.
const TEMP_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Per-sensor read timeout. Each `read_base_temps` call does three sequential
/// reads, so worst-case wall time is 3x this if all sensors fail to respond.
const TEMP_READ_DEADLINE: Duration = Duration::from_millis(300);
/// `recv_timeout` window so the loop wakes regularly to do thermal polling
/// and idle-detection even when no telemetry is arriving.
const RECV_TIMEOUT: Duration = Duration::from_millis(250);

// Approximate "elevated" thresholds. Moza firmware auto-protects somewhere
// above these; the goal is to surface unusual heat well before that fires.
const MCU_TEMP_WARN_C: f32 = 75.0;
const MOSFET_TEMP_WARN_C: f32 = 70.0;
const MOTOR_TEMP_WARN_C: f32 = 95.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameId {
    Wreckfest2,
    /// Any Codemasters EGO-engine title using the "extradata" UDP format
    /// (DiRT 2/3/Showdown, F1 2010-2017, GRID series, DiRT Rally, DR2).
    CodemastersLegacy,
    /// BeamNG.drive (also Live For Speed and other LFS-OutGauge-compat
    /// titles, as long as they target the same UDP port).
    BeamNG,
    /// Automobilista 2 / Project CARS 2 (and PC1, PC3) — Madness-engine
    /// PC2 UDP format.
    Ams2,
    /// Assetto Corsa 1 — handshake-based UDP, adaptive redline.
    AssettoCorsa,
}

impl GameId {
    /// Compact tag for the per-tick status line (`[WF2]`, `[AC]`, …).
    fn label(self) -> &'static str {
        match self {
            GameId::Wreckfest2 => "WF2",
            GameId::CodemastersLegacy => "DR2",
            GameId::BeamNG => "BNG",
            GameId::Ams2 => "AMS2",
            GameId::AssettoCorsa => "AC",
        }
    }

    /// Full name for startup / lifecycle log lines.
    fn name(self) -> &'static str {
        match self {
            GameId::Wreckfest2 => "Wreckfest 2",
            GameId::CodemastersLegacy => "Codemasters EGO (DR2 / DiRT / F1 / GRID)",
            GameId::BeamNG => "BeamNG.drive",
            GameId::Ams2 => "Automobilista 2 / Project CARS 2",
            GameId::AssettoCorsa => "Assetto Corsa",
        }
    }
}

struct Update {
    game: GameId,
    engine: EngineState,
}

/// Open the Moza wheelbase, honouring an explicit `--serial` if the user
/// supplied one or autodetecting otherwise. Returns `None` if no wheel is
/// reachable; the caller chooses whether that's fatal (startup) or just a
/// reconnect-attempt failure (mid-run after a USB unplug).
fn open_wheel(
    user_serial: Option<&str>,
    user_protocol: Option<Protocol>,
) -> Option<(String, Protocol, Moza)> {
    let path = match user_serial {
        Some(p) => p.to_string(),
        None => moza::find_wheelbase()?,
    };
    let protocol = user_protocol.unwrap_or_else(|| moza::detect_protocol(&path));
    match Moza::open(&path, protocol) {
        Ok(w) => Some((path, protocol, w)),
        Err(e) => {
            debug!("open Moza at {path}: {e}");
            None
        }
    }
}

pub fn run(args: ListenArgs) -> ExitCode {
    let user_serial = args.serial.clone();
    let user_protocol = args.protocol.map(Protocol::from);

    let (serial_path, protocol, initial_wheel) =
        match open_wheel(user_serial.as_deref(), user_protocol) {
            Some(t) => t,
            None => {
                error!(
                    "no Moza wheelbase found under /dev/serial/by-id/ (looking for *Gudsen*Base*). \
                     plug it in, or pass --serial /dev/ttyACMx"
                );
                return ExitCode::FAILURE;
            }
        };
    info!("opening Moza wheelbase at {serial_path} ({protocol:?} protocol)");
    let mut wheel: Option<Moza> = Some(initial_wheel);
    let mut last_reconnect = Instant::now();

    let (tx, rx) = mpsc::channel::<Update>();
    if !spawn_listener(args.wf2_port, GameId::Wreckfest2, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !spawn_listener(args.dr2_port, GameId::CodemastersLegacy, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !spawn_listener(args.ams2_port, GameId::Ams2, tx.clone()) {
        return ExitCode::FAILURE;
    }
    if !spawn_beamng_listener(args.beamng_port, tx.clone()) {
        return ExitCode::FAILURE;
    }
    spawn_ac_listener(args.ac_port, tx);

    let led_count = args.leds;
    let verbose = args.verbose;

    // Only do the in-place `\r` rewrite when stdout is a real TTY. Piped
    // output (`moza-rev | tee log.txt`, systemd, journald, etc.) gets
    // newline-terminated lines instead.
    let inplace_status = !verbose && std::io::stdout().is_terminal();

    let mut last_bitmask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();
    let mut last_temp_poll = Instant::now() - TEMP_POLL_INTERVAL; // poll once on startup
    let mut active_game: Option<GameId> = None;
    let mut last_packet_at: Option<Instant> = None;
    let mut packets_since_status: u32 = 0;

    loop {
        // Reconnect attempt if the wheel got unplugged. Quiet at debug
        // until the open succeeds — info-level chatter every 3s would
        // be noisy. The transition log is in `open_wheel`'s caller.
        if wheel.is_none() && last_reconnect.elapsed() >= RECONNECT_INTERVAL {
            if let Some((path, _, w)) = open_wheel(user_serial.as_deref(), user_protocol) {
                info!("Moza wheelbase reconnected at {path}");
                wheel = Some(w);
                // Force the next heartbeat to re-send the bitmask so the
                // bar matches engine state immediately rather than waiting
                // for the next change.
                last_bitmask = None;
            }
            last_reconnect = Instant::now();
        }

        if last_temp_poll.elapsed() >= TEMP_POLL_INTERVAL {
            if let Some(w) = wheel.as_mut() {
                poll_temps(w);
            }
            last_temp_poll = Instant::now();
        }

        // Idle timeout: clear the active-game state so the next heartbeat
        // writes 0 to the bar.
        if let Some(t) = last_packet_at
            && t.elapsed() >= IDLE_TIMEOUT
            && active_game.is_some()
        {
            info!("no telemetry for {:?} — going idle", IDLE_TIMEOUT);
            active_game = None;
        }

        // Heartbeat keepalive. Always write at least once per
        // HEARTBEAT_INTERVAL, even with no telemetry — this keeps the
        // bar refreshed AND surfaces a USB unplug as a write error
        // within ~1-2s, which would otherwise go unnoticed in idle mode.
        if last_send.elapsed() >= HEARTBEAT_INTERVAL {
            let bitmask = if active_game.is_some() {
                last_bitmask.unwrap_or(0)
            } else {
                0
            };
            wheel = try_write(wheel.take(), bitmask, led_count, &mut last_reconnect);
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        let update = match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(u) => u,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                error!("all listener threads exited; shutting down");
                return ExitCode::FAILURE;
            }
        };

        if active_game != Some(update.game) {
            info!("active game: {}", update.game.name());
            active_game = Some(update.game);
        }
        last_packet_at = Some(Instant::now());
        packets_since_status += 1;

        let bitmask = rpm_to_bitmask(&update.engine, led_count);
        if Some(bitmask) != last_bitmask {
            wheel = try_write(wheel.take(), bitmask, led_count, &mut last_reconnect);
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        if verbose {
            print_status(
                update.game,
                &update.engine,
                bitmask,
                led_count,
                packets_since_status,
                false, // verbose: one line per packet, never overwrite
            );
            packets_since_status = 0;
        } else if last_status.elapsed() >= STATUS_INTERVAL {
            print_status(
                update.game,
                &update.engine,
                bitmask,
                led_count,
                packets_since_status,
                inplace_status,
            );
            last_status = Instant::now();
            packets_since_status = 0;
        }
    }
}

/// Spawn a UDP listener thread for `game` on `port`. Returns true on
/// successful bind, false if the port couldn't be claimed (caller should
/// fail-fast in that case).
fn spawn_listener(port: u16, game: GameId, tx: mpsc::Sender<Update>) -> bool {
    let bind_addr = format!("0.0.0.0:{port}");
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind {} {bind_addr}: {e}", game.label());
            return false;
        }
    };
    info!(
        "listening for {} telemetry on udp://{bind_addr}",
        game.name()
    );
    thread::Builder::new()
        .name(format!("listener-{}", game.label()))
        .spawn(move || listener_loop(socket, game, tx))
        .expect("failed to spawn listener thread");
    true
}

fn listener_loop(socket: UdpSocket, game: GameId, tx: mpsc::Sender<Update>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("{} recv: {e}", game.label());
                continue;
            }
        };
        let Some(engine) = parse(game, &buf[..n]) else {
            continue;
        };
        if tx.send(Update { game, engine }).is_err() {
            // Receiver dropped → main has exited.
            return;
        }
    }
}

fn parse(game: GameId, buf: &[u8]) -> Option<EngineState> {
    match game {
        GameId::Wreckfest2 => wreckfest::parse_main(buf),
        GameId::CodemastersLegacy => {
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
        GameId::Ams2 => {
            // Madness engine multiplexes 9 packet types on the same port;
            // TelemetryPacket::from_bytes filters to type 0 (telemetry).
            let pkt = madness::TelemetryPacket::from_bytes(buf)?;
            let redline = pkt.data.redline_rpm();
            // Skip menu/loading frames where the engine isn't initialized.
            if redline <= 0 {
                return None;
            }
            Some(EngineState {
                rpm: pkt.data.rpm(),
                rpm_redline: redline,
                rpm_idle: AMS2_ASSUMED_IDLE,
            })
        }
        // BeamNG and Assetto Corsa run on dedicated listeners that
        // maintain adaptive redline state — see `listener_loop_outgauge`
        // and `listener_loop_ac`.
        GameId::BeamNG | GameId::AssettoCorsa => None,
    }
}

/// BeamNG's OutGauge format gives us current RPM but no redline or idle
/// values. We track the highest RPM seen in the session and use that as
/// an adaptive redline; idle is fixed at a typical petrol-car value.
/// This means the LED bar self-tunes after a few revs of the engine.
fn spawn_beamng_listener(port: u16, tx: mpsc::Sender<Update>) -> bool {
    let bind_addr = format!("0.0.0.0:{port}");
    let socket = match UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("bind BNG {bind_addr}: {e}");
            return false;
        }
    };
    info!(
        "listening for {} telemetry on udp://{bind_addr}",
        GameId::BeamNG.name()
    );
    thread::Builder::new()
        .name(format!("listener-{}", GameId::BeamNG.label()))
        .spawn(move || listener_loop_outgauge(socket, tx))
        .expect("failed to spawn BNG listener thread");
    true
}

fn listener_loop_outgauge(socket: UdpSocket, tx: mpsc::Sender<Update>) {
    let mut max_rpm: f32 = BEAMNG_INITIAL_REDLINE;
    let mut buf = vec![0u8; 256];
    loop {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("BNG recv: {e}");
                continue;
            }
        };
        let Some(packet) = outgauge::Packet::from_bytes(&buf[..n]) else {
            continue;
        };
        let rpm_f = { packet.rpm };
        // Skip when engine is off / car not loaded.
        if rpm_f <= 1.0 {
            continue;
        }
        if rpm_f > max_rpm {
            max_rpm = rpm_f;
        }
        let engine = EngineState {
            rpm: rpm_f as i32,
            rpm_redline: max_rpm as i32,
            rpm_idle: BEAMNG_ASSUMED_IDLE,
        };
        if tx
            .send(Update {
                game: GameId::BeamNG,
                engine,
            })
            .is_err()
        {
            return;
        }
    }
}

/// Assetto Corsa is request-response: the client must send a Handshake
/// to AC's listener port and receive a HandshakeResponse before AC
/// streams anything. We retry the handshake periodically so it doesn't
/// matter whether AC is started before or after moza-rev — whenever a
/// session loads, this loop will pick it up. RTCarInfo has no redline
/// field, so we adapt like BeamNG does.
fn spawn_ac_listener(port: u16, tx: mpsc::Sender<Update>) {
    let target = format!("127.0.0.1:{port}");
    info!(
        "listening for {} telemetry via udp://{target} (handshake retry every {}s)",
        GameId::AssettoCorsa.name(),
        AC_HANDSHAKE_RETRY.as_secs()
    );
    thread::Builder::new()
        .name(format!("listener-{}", GameId::AssettoCorsa.label()))
        .spawn(move || listener_loop_ac(target, tx))
        .expect("failed to spawn AC listener thread");
}

fn listener_loop_ac(target: String, tx: mpsc::Sender<Update>) {
    let mut buf = vec![0u8; 1024];
    let mut max_rpm: f32 = AC_INITIAL_REDLINE;

    loop {
        // Fresh socket per cycle so AC sees a new client. The previous
        // session's ephemeral port may already have a queued Dismiss.
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(e) => {
                error!("AC bind: {e}");
                thread::sleep(AC_HANDSHAKE_RETRY);
                continue;
            }
        };
        if let Err(e) = socket.connect(&target) {
            debug!("AC connect {target}: {e}");
            thread::sleep(AC_HANDSHAKE_RETRY);
            continue;
        }
        if let Err(e) = socket.set_read_timeout(Some(AC_HANDSHAKE_TIMEOUT)) {
            error!("AC set_read_timeout: {e}");
            thread::sleep(AC_HANDSHAKE_RETRY);
            continue;
        }

        if let Err(e) = socket.send(&Handshake::new(op::HANDSHAKE).to_bytes()) {
            debug!("AC send handshake: {e}");
            thread::sleep(AC_HANDSHAKE_RETRY);
            continue;
        }
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                // Most common case: AC not running, or running but no
                // session loaded. Quiet at debug level — info-level
                // chatter every 3s would be noisy.
                debug!("AC handshake: no reply, retrying");
                thread::sleep(AC_HANDSHAKE_RETRY);
                continue;
            }
        };
        let Some(hs) = HandshakeResponse::from_bytes(&buf[..n]) else {
            warn!("AC handshake reply too short ({n} bytes); retrying");
            thread::sleep(AC_HANDSHAKE_RETRY);
            continue;
        };
        info!(
            "AC connected: car={:?} track={:?} driver={:?}",
            hs.car(),
            hs.track(),
            hs.driver()
        );

        if let Err(e) = socket.send(&Handshake::new(op::SUBSCRIBE_UPDATE).to_bytes()) {
            warn!("AC subscribe: {e}");
            continue;
        }
        if let Err(e) = socket.set_read_timeout(Some(AC_STREAM_TIMEOUT)) {
            warn!("AC set stream timeout: {e}");
            continue;
        }

        // Stream loop. On timeout we treat it as session ended and go
        // back to handshake retry — covers menu return, quit, etc. We
        // still receive every packet (so the stream-timeout watchdog
        // works) but only forward at ~60 Hz to avoid spamming the
        // channel at AC's full 333 Hz physics rate.
        let mut last_forwarded = Instant::now()
            .checked_sub(AC_FORWARD_INTERVAL)
            .unwrap_or_else(Instant::now);
        loop {
            let n = match socket.recv(&mut buf) {
                Ok(n) => n,
                Err(_) => {
                    info!("AC stream timed out; returning to handshake retry");
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
            if last_forwarded.elapsed() < AC_FORWARD_INTERVAL {
                continue;
            }
            last_forwarded = Instant::now();
            let engine = EngineState {
                rpm: rpm_f as i32,
                rpm_redline: max_rpm as i32,
                rpm_idle: AC_ASSUMED_IDLE,
            };
            if tx
                .send(Update {
                    game: GameId::AssettoCorsa,
                    engine,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

/// Send a bitmask to the wheel, dropping the handle on failure so the
/// next loop tick triggers a reconnect attempt. Returns the handle on
/// success, `None` on failure or if `wheel` was already `None`.
fn try_write(
    wheel: Option<Moza>,
    bitmask: u32,
    led_count: usize,
    last_reconnect: &mut Instant,
) -> Option<Moza> {
    let mut w = wheel?;
    match w.send_rpm_bitmask(bitmask, led_count) {
        Ok(()) => Some(w),
        Err(e) => {
            warn!("Moza wheelbase write failed ({e}); will retry to reconnect");
            *last_reconnect = Instant::now();
            None
        }
    }
}

fn poll_temps(wheel: &mut Moza) {
    match wheel.read_base_temps(TEMP_READ_DEADLINE) {
        Ok(t) => log_temps(&t),
        Err(e) => debug!("temp read failed: {e}"),
    }
}

fn log_temps(t: &BaseTemps) {
    debug!(
        "temps  MCU={} MOSFET={} motor={}",
        fmt_temp(t.mcu_c),
        fmt_temp(t.mosfet_c),
        fmt_temp(t.motor_c)
    );
    if let Some(c) = t.mcu_c
        && c >= MCU_TEMP_WARN_C
    {
        warn!("MCU temp elevated: {c:.1}°C (>= {MCU_TEMP_WARN_C:.0}°C)");
    }
    if let Some(c) = t.mosfet_c
        && c >= MOSFET_TEMP_WARN_C
    {
        warn!("MOSFET temp elevated: {c:.1}°C (>= {MOSFET_TEMP_WARN_C:.0}°C)");
    }
    if let Some(c) = t.motor_c
        && c >= MOTOR_TEMP_WARN_C
    {
        warn!("motor temp elevated: {c:.1}°C (>= {MOTOR_TEMP_WARN_C:.0}°C)");
    }
}

fn fmt_temp(t: Option<f32>) -> String {
    t.map_or_else(|| "?".to_string(), |c| format!("{c:.1}°C"))
}

fn print_status(
    game: GameId,
    engine: &EngineState,
    bitmask: u32,
    led_count: usize,
    packets: u32,
    inplace: bool,
) {
    let bar = led_bar(bitmask, led_count);
    let line = format!(
        "[{}] rpm {:>5} / {:>5}  idle {:>4}  [{}]  {} pkt",
        game.label(),
        engine.rpm,
        engine.rpm_redline,
        engine.rpm_idle,
        bar,
        packets
    );
    if inplace {
        // Overwrite a single status line in place; trailing spaces clear
        // any leftover from a previous longer line.
        print!("\r{line}    ");
        let _ = std::io::stdout().flush();
    } else {
        println!("{line}");
    }
}

fn led_bar(bitmask: u32, led_count: usize) -> String {
    (0..led_count)
        .map(|i| {
            if bitmask & (1 << i) != 0 {
                '●'
            } else {
                '○'
            }
        })
        .collect()
}

fn rpm_to_bitmask(engine: &EngineState, led_count: usize) -> u32 {
    if engine.rpm_redline <= engine.rpm_idle || led_count == 0 {
        return 0;
    }
    let span = (engine.rpm_redline - engine.rpm_idle) as f32;
    let above_idle = (engine.rpm - engine.rpm_idle).max(0) as f32;
    let frac = (above_idle / span).clamp(0.0, 1.0);
    let lit = (frac * led_count as f32).round() as usize;
    let lit = lit.min(led_count);
    if lit == 0 {
        0
    } else if lit >= 32 {
        u32::MAX
    } else {
        (1u32 << lit) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(rpm: i32) -> EngineState {
        EngineState {
            rpm,
            rpm_redline: 6500,
            rpm_idle: 800,
        }
    }

    #[test]
    fn below_idle_lights_nothing() {
        assert_eq!(rpm_to_bitmask(&engine(500), 10), 0);
    }

    #[test]
    fn at_redline_lights_everything() {
        assert_eq!(rpm_to_bitmask(&engine(6500), 10), 0x3FF);
    }

    #[test]
    fn above_redline_clamps() {
        assert_eq!(rpm_to_bitmask(&engine(9000), 10), 0x3FF);
    }

    #[test]
    fn midband_lights_half() {
        // (3650 - 800) / (6500 - 800) = 0.5 → 5 of 10 LEDs
        assert_eq!(rpm_to_bitmask(&engine(3650), 10), 0x1F);
    }

    #[test]
    fn unset_engine_data_is_safe() {
        let zero = EngineState {
            rpm: 0,
            rpm_redline: 0,
            rpm_idle: 0,
        };
        assert_eq!(rpm_to_bitmask(&zero, 10), 0);
    }
}
