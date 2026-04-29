use std::env;
use std::io::{IsTerminal, Write};
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};

use moza_rev::assetto_corsa::{self, Handshake, HandshakeResponse, RtCarInfo, op};
use moza_rev::codemasters_legacy::{self, Telemetry};
use moza_rev::configure;
use moza_rev::madness;
use moza_rev::moza::{self, BaseTemps, Moza, Protocol};
use moza_rev::outgauge;
use moza_rev::wreckfest::{self, EngineState};

const DEFAULT_WF2_PORT: u16 = 23123;
const DEFAULT_DR2_PORT: u16 = codemasters_legacy::DEFAULT_PORT; // 20777
const DEFAULT_BEAMNG_PORT: u16 = outgauge::DEFAULT_PORT; // 4444
const DEFAULT_AMS2_PORT: u16 = madness::DEFAULT_PORT; // 5606
const DEFAULT_AC_PORT: u16 = assetto_corsa::DEFAULT_PORT; // 9996
const DEFAULT_LED_COUNT: usize = 10;
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

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Subcommands. Anything else (including no args) drops into the
    // listener flow with the existing flag parser.
    if env::args().nth(1).as_deref() == Some("configure") {
        return configure::run();
    }

    let args = match Args::from_env() {
        Ok(a) => a,
        Err(msg) => {
            error!("{msg}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let serial_path = match args.serial_path.or_else(moza::find_wheelbase) {
        Some(p) => p,
        None => {
            error!(
                "no Moza wheelbase found under /dev/serial/by-id/ (looking for *Gudsen*Base*). \
                 plug it in, or pass --serial /dev/ttyACMx"
            );
            return ExitCode::FAILURE;
        }
    };
    let protocol = args
        .protocol
        .unwrap_or_else(|| moza::detect_protocol(&serial_path));
    info!("opening Moza wheelbase at {serial_path} ({protocol:?} protocol)");
    let mut wheel = match Moza::open(&serial_path, protocol) {
        Ok(m) => m,
        Err(e) => {
            error!("{e}");
            return ExitCode::FAILURE;
        }
    };

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

    // Only do the in-place `\r` rewrite when stdout is a real TTY. Piped
    // output (`moza-rev | tee log.txt`, systemd, journald, etc.) gets
    // newline-terminated lines instead.
    let inplace_status = !args.verbose && std::io::stdout().is_terminal();

    let mut last_bitmask: Option<u32> = None;
    let mut last_send = Instant::now();
    let mut last_status = Instant::now();
    let mut last_temp_poll = Instant::now() - TEMP_POLL_INTERVAL; // poll once on startup
    let mut active_game: Option<GameId> = None;
    let mut last_packet_at: Option<Instant> = None;
    let mut packets_since_status: u32 = 0;

    loop {
        if last_temp_poll.elapsed() >= TEMP_POLL_INTERVAL {
            poll_temps(&mut wheel);
            last_temp_poll = Instant::now();
        }

        // Idle timeout: if the active game stops sending, clear the bar so
        // the wheel returns to its breathing pattern.
        if let Some(t) = last_packet_at
            && t.elapsed() >= IDLE_TIMEOUT
            && active_game.is_some()
        {
            info!("no telemetry for {:?} — going idle", IDLE_TIMEOUT);
            active_game = None;
            last_bitmask = None;
            let _ = wheel.send_rpm_bitmask(0, args.led_count);
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

        let bitmask = rpm_to_bitmask(&update.engine, args.led_count);
        let changed = Some(bitmask) != last_bitmask;
        let stale = last_send.elapsed() >= HEARTBEAT_INTERVAL;
        if changed || stale {
            if let Err(e) = wheel.send_rpm_bitmask(bitmask, args.led_count) {
                error!("write to wheel: {e}");
                continue;
            }
            last_bitmask = Some(bitmask);
            last_send = Instant::now();
        }

        if args.verbose {
            print_status(
                update.game,
                &update.engine,
                bitmask,
                args.led_count,
                packets_since_status,
                false, // verbose: one line per packet, never overwrite
            );
            packets_since_status = 0;
        } else if last_status.elapsed() >= STATUS_INTERVAL {
            print_status(
                update.game,
                &update.engine,
                bitmask,
                args.led_count,
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

struct Args {
    wf2_port: u16,
    dr2_port: u16,
    ams2_port: u16,
    beamng_port: u16,
    ac_port: u16,
    serial_path: Option<String>,
    led_count: usize,
    protocol: Option<Protocol>,
    verbose: bool,
}

impl Args {
    fn from_env() -> Result<Self, String> {
        let mut wf2_port = DEFAULT_WF2_PORT;
        let mut dr2_port = DEFAULT_DR2_PORT;
        let mut ams2_port = DEFAULT_AMS2_PORT;
        let mut beamng_port = DEFAULT_BEAMNG_PORT;
        let mut ac_port = DEFAULT_AC_PORT;
        let mut serial_path = None;
        let mut led_count = DEFAULT_LED_COUNT;
        let mut protocol = None;
        let mut verbose = false;

        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--wf2-port" => {
                    let v = it.next().ok_or("--wf2-port needs a value")?;
                    wf2_port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--dr2-port" => {
                    let v = it.next().ok_or("--dr2-port needs a value")?;
                    dr2_port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--ams2-port" => {
                    let v = it.next().ok_or("--ams2-port needs a value")?;
                    ams2_port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--beamng-port" => {
                    let v = it.next().ok_or("--beamng-port needs a value")?;
                    beamng_port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--ac-port" => {
                    let v = it.next().ok_or("--ac-port needs a value")?;
                    ac_port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
                }
                "--serial" | "-s" => {
                    serial_path = Some(it.next().ok_or("--serial needs a path")?);
                }
                "--leds" | "-l" => {
                    let v = it.next().ok_or("--leds needs a value")?;
                    led_count = v.parse().map_err(|_| format!("invalid led count: {v}"))?;
                }
                "--protocol" => {
                    let v = it.next().ok_or("--protocol needs a value")?;
                    protocol = Some(match v.as_str() {
                        "modern" | "new" => Protocol::Modern,
                        "legacy" | "old" | "es" => Protocol::Legacy,
                        other => {
                            return Err(format!(
                                "invalid protocol (expected modern|legacy): {other}"
                            ));
                        }
                    });
                }
                "--verbose" | "-v" => {
                    verbose = true;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Self {
            wf2_port,
            dr2_port,
            ams2_port,
            beamng_port,
            ac_port,
            serial_path,
            led_count,
            protocol,
            verbose,
        })
    }
}

fn print_usage() {
    eprintln!(
        "moza-rev — drive the Moza wheel's RPM LED bar from game telemetry\n\
         \n\
         usage: moza-rev [--wf2-port PORT] [--dr2-port PORT] [--ams2-port PORT]\n\
                          [--beamng-port PORT] [--ac-port PORT] [--serial /dev/ttyACMx]\n\
                          [--leds N] [--protocol modern|legacy] [-v]\n\
                moza-rev configure   # detect installed games and offer to enable telemetry\n\
         \n\
         defaults: --wf2-port {DEFAULT_WF2_PORT} (Wreckfest 2 \"Pino\")\n\
                   --dr2-port {DEFAULT_DR2_PORT} (Codemasters legacy: DR2, DR1, F1 2010-2017,\n\
                                                  Dirt 2/3/Showdown, GRID series)\n\
                   --ams2-port {DEFAULT_AMS2_PORT} (AMS2 / Project CARS 2 / PC1 / PC3 — see README\n\
                                                  for the iptables loopback fix on Linux)\n\
                   --beamng-port {DEFAULT_BEAMNG_PORT} (BeamNG.drive OutGauge / LFS)\n\
                   --ac-port {DEFAULT_AC_PORT} (Assetto Corsa 1 — handshake-based, adaptive redline)\n\
                   --leds {DEFAULT_LED_COUNT}, serial+protocol autodetected.\n\
         \n\
         Both listeners run simultaneously. The active game is whichever\n\
         was last to send a packet; the wheel goes idle after 2s of silence.\n\
         \n\
         Wreckfest 2: enable telemetry by setting \"enabled\": 1 in:\n\
           ~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/\n\
             compatdata/1203190/pfx/drive_c/users/steamuser/Documents/My Games/\n\
             Wreckfest 2/<userid>/savegame/telemetry/config.json\n\
         \n\
         DiRT Rally 2.0: edit hardware_settings_config.xml, set the <udp> child\n\
           of <motion_platform> to enabled=\"true\" extradata=\"3\"."
    );
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
