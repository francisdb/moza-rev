// Assetto Corsa Competizione "Broadcasting" UDP protocol. ACC's only
// network telemetry surface — connection-oriented (UDP, but with a
// register/result handshake), spectator/UI oriented. Carries gear,
// speed, lap, position, weather, session phase. Does NOT carry engine
// RPM — that lives in ACC's Windows-only shared-memory API. The parser
// and example here exist as protocol groundwork; driving the LED bar
// from ACC would still need a Windows-side shmem→UDP bridge for RPM.
//
// Wire format: every datagram is `[msg_type:u8] [body...]`, all multi-
// byte fields little-endian, strings UTF-8 with a `u16` length prefix.
// Reference: `BroadcastingNetworkProtocol.cs` shipped in ACC's Dedicated
// Server SDK (e.g. mirrored at angel-git/acc-broadcasting).
//
// Setup: ACC must have `Documents/Assetto Corsa Competizione/Config/
// broadcasting.json` set with `udpListenerPort: 9000` and a
// `connectionPassword`, then ACC restarted. The file is UTF-16 LE.

pub const DEFAULT_PORT: u16 = 9000;
/// Broadcasting protocol version. Sent in REGISTER; ACC rejects with
/// "Your broadcasting control app is outdated" if too low. Older
/// references (e.g. angel-git/acc-broadcasting circa 2020) used 2;
/// current ACC requires 4.
pub const PROTOCOL_VERSION: u8 = 4;
/// Default `connectionPassword` we propose in `configure` and use as
/// the example's fallback when `--password` isn't given. Memorable
/// rather than secure — broadcasting is local-only and the password's
/// purpose is just to stop accidental cross-tool collisions.
pub const DEFAULT_PASSWORD: &str = "moza";

/// Outbound (client → ACC) message-type bytes.
pub mod outbound {
    pub const REGISTER_COMMAND_APPLICATION: u8 = 1;
    pub const UNREGISTER_COMMAND_APPLICATION: u8 = 9;
    pub const REQUEST_ENTRY_LIST: u8 = 10;
    pub const REQUEST_TRACK_DATA: u8 = 11;
}

/// Inbound (ACC → client) message-type bytes.
pub mod inbound {
    pub const REGISTRATION_RESULT: u8 = 1;
    pub const REALTIME_UPDATE: u8 = 2;
    pub const REALTIME_CAR_UPDATE: u8 = 3;
    pub const ENTRY_LIST: u8 = 4;
    pub const TRACK_DATA: u8 = 5;
    pub const ENTRY_LIST_CAR: u8 = 6;
    pub const BROADCASTING_EVENT: u8 = 7;
}

/// Build a REGISTER_COMMAND_APPLICATION datagram. ACC replies with
/// REGISTRATION_RESULT and then starts streaming REALTIME_UPDATE +
/// REALTIME_CAR_UPDATE at `update_interval_ms`.
///
/// `command_password` may be empty for read-only spectator access — the
/// game will still register us and stream telemetry but reject any
/// outbound CHANGE_HUD_PAGE / CHANGE_FOCUS / replay commands.
pub fn build_register(
    display_name: &str,
    connection_password: &str,
    update_interval_ms: i32,
    command_password: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        2 + 6 + display_name.len() + connection_password.len() + command_password.len() + 4,
    );
    out.push(outbound::REGISTER_COMMAND_APPLICATION);
    out.push(PROTOCOL_VERSION);
    write_string(&mut out, display_name);
    write_string(&mut out, connection_password);
    out.extend_from_slice(&update_interval_ms.to_le_bytes());
    write_string(&mut out, command_password);
    out
}

/// Build an UNREGISTER_COMMAND_APPLICATION datagram. Send before exit
/// so ACC drops us from its broadcast list cleanly.
pub fn build_unregister(connection_id: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.push(outbound::UNREGISTER_COMMAND_APPLICATION);
    out.extend_from_slice(&connection_id.to_le_bytes());
    out
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

/// Parsed inbound message. `Other` covers types we don't decode but
/// still want to surface (entry list, track data, broadcast events).
#[derive(Debug, Clone)]
pub enum Message {
    RegistrationResult(RegistrationResult),
    RealtimeUpdate(RealtimeUpdate),
    RealtimeCarUpdate(RealtimeCarUpdate),
    Other { msg_type: u8, body_len: usize },
}

#[derive(Debug, Clone)]
pub struct RegistrationResult {
    pub connection_id: i32,
    pub success: bool,
    /// True when registered without a command password (spectator).
    pub readonly: bool,
    pub error_message: String,
}

#[derive(Debug, Clone)]
pub struct RealtimeUpdate {
    pub event_index: u16,
    pub session_index: u16,
    pub session_type: u8,
    pub phase: u8,
    /// Milliseconds since session start.
    pub session_time_ms: f32,
    /// Milliseconds remaining (for timed sessions).
    pub session_end_time_ms: f32,
    pub focused_car_index: i32,
    pub active_camera_set: String,
    pub active_camera: String,
    pub current_hud_page: String,
    pub is_replay_playing: bool,
    /// Replay timeline position in milliseconds (only set when replaying).
    pub replay_session_time_ms: Option<i32>,
    /// Replay remaining time in milliseconds (only set when replaying).
    pub replay_remaining_time_ms: Option<i32>,
    /// Time of day as seconds since midnight; subject to ACC's race-time
    /// multiplier so a real second of wall-clock can be 60 game seconds.
    pub time_of_day_seconds: f32,
    pub ambient_temp_c: i8,
    pub track_temp_c: i8,
    /// 0.0–1.0; wire is `byte / 10`.
    pub clouds: f32,
    pub rain_level: f32,
    pub wetness: f32,
    pub best_session_lap: LapInfo,
}

impl RealtimeUpdate {
    /// hh:mm extracted from `time_of_day_seconds` (wraps at 24h).
    pub fn time_of_day(&self) -> (u8, u8) {
        let total_secs = self.time_of_day_seconds.max(0.0) as u32;
        (
            ((total_secs / 3600) % 24) as u8,
            ((total_secs / 60) % 60) as u8,
        )
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeCarUpdate {
    pub car_index: u16,
    pub driver_index: u16,
    /// Total drivers declared for this car (matches `EntryListCar.drivers`).
    /// Added in protocol v3+; absent in v2.
    pub driver_count: u8,
    /// Raw - 2: -2 = engine off / no gear, -1 = R, 0 = N, 1+ = forward.
    pub gear: i8,
    pub world_pos_x: f32,
    pub world_pos_y: f32,
    pub yaw: f32,
    pub car_location: u8,
    pub kmh: u16,
    pub position: u16,
    pub cup_position: u16,
    pub track_position: u16,
    /// 0.0–1.0 around the lap.
    pub spline_position: f32,
    pub laps: u16,
    /// Milliseconds vs best-session lap; positive = slower.
    pub delta_ms: i32,
    pub best_session_lap: LapInfo,
    pub last_lap: LapInfo,
    pub current_lap: LapInfo,
}

impl RealtimeCarUpdate {
    pub fn gear_label(&self) -> String {
        match self.gear {
            -2 => "?".to_string(),
            -1 => "R".to_string(),
            0 => "N".to_string(),
            n => n.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LapInfo {
    pub lap_time_ms: i32,
    pub car_index: u16,
    pub driver_index: u16,
    pub splits_ms: Vec<i32>,
    pub is_invalid: bool,
    pub is_valid_for_best: bool,
    pub is_outlap: bool,
    pub is_inlap: bool,
}

impl LapInfo {
    /// `i32::MAX` is ACC's "no lap recorded yet" sentinel.
    pub fn is_recorded(&self) -> bool {
        self.lap_time_ms != i32::MAX && self.lap_time_ms > 0
    }

    /// Format as `mm:ss.mmm` or `--:--.---` for unrecorded laps.
    pub fn format(&self) -> String {
        if !self.is_recorded() {
            return "--:--.---".to_string();
        }
        let total = self.lap_time_ms as u32;
        let m = total / 60_000;
        let s = (total / 1000) % 60;
        let ms = total % 1000;
        format!("{m:02}:{s:02}.{ms:03}")
    }
}

pub fn parse_message(buf: &[u8]) -> Option<Message> {
    let mut r = Reader::new(buf);
    let msg_type = r.read_u8()?;
    match msg_type {
        inbound::REGISTRATION_RESULT => {
            Some(Message::RegistrationResult(parse_registration(&mut r)?))
        }
        inbound::REALTIME_UPDATE => Some(Message::RealtimeUpdate(parse_realtime_update(&mut r)?)),
        inbound::REALTIME_CAR_UPDATE => Some(Message::RealtimeCarUpdate(
            parse_realtime_car_update(&mut r)?,
        )),
        other => Some(Message::Other {
            msg_type: other,
            body_len: buf.len().saturating_sub(1),
        }),
    }
}

fn parse_registration(r: &mut Reader) -> Option<RegistrationResult> {
    let connection_id = r.read_i32_le()?;
    let success = r.read_u8()? > 0;
    // `IsReadonly` is reported as `read == 0` in the SDK reference: when
    // the client supplies no command password, ACC sends 0 here.
    let readonly = r.read_u8()? == 0;
    let error_message = r.read_string()?;
    Some(RegistrationResult {
        connection_id,
        success,
        readonly,
        error_message,
    })
}

fn parse_realtime_update(r: &mut Reader) -> Option<RealtimeUpdate> {
    let event_index = r.read_u16_le()?;
    let session_index = r.read_u16_le()?;
    let session_type = r.read_u8()?;
    let phase = r.read_u8()?;
    let session_time_ms = r.read_f32_le()?;
    let session_end_time_ms = r.read_f32_le()?;
    let focused_car_index = r.read_i32_le()?;
    let active_camera_set = r.read_string()?;
    let active_camera = r.read_string()?;
    let current_hud_page = r.read_string()?;
    let is_replay_playing = r.read_u8()? > 0;
    // v4 replay block: 2 × i32. Older v2 spec had 2 × f32 + 1 × i32 here.
    let (replay_session_time_ms, replay_remaining_time_ms) = if is_replay_playing {
        (Some(r.read_i32_le()?), Some(r.read_i32_le()?))
    } else {
        (None, None)
    };
    let time_of_day_seconds = r.read_f32_le()?;
    let ambient_temp_c = r.read_i8()?;
    let track_temp_c = r.read_i8()?;
    let clouds = r.read_u8()? as f32 / 10.0;
    let rain_level = r.read_u8()? as f32 / 10.0;
    let wetness = r.read_u8()? as f32 / 10.0;
    let best_session_lap = parse_lap(r)?;
    Some(RealtimeUpdate {
        event_index,
        session_index,
        session_type,
        phase,
        session_time_ms,
        session_end_time_ms,
        focused_car_index,
        active_camera_set,
        active_camera,
        current_hud_page,
        is_replay_playing,
        replay_session_time_ms,
        replay_remaining_time_ms,
        time_of_day_seconds,
        ambient_temp_c,
        track_temp_c,
        clouds,
        rain_level,
        wetness,
        best_session_lap,
    })
}

fn parse_realtime_car_update(r: &mut Reader) -> Option<RealtimeCarUpdate> {
    let car_index = r.read_u16_le()?;
    let driver_index = r.read_u16_le()?;
    // v4-only field; v2 went straight from driver_index to gear.
    let driver_count = r.read_u8()?;
    let gear = (r.read_u8()? as i16 - 2) as i8;
    let world_pos_x = r.read_f32_le()?;
    let world_pos_y = r.read_f32_le()?;
    let yaw = r.read_f32_le()?;
    let car_location = r.read_u8()?;
    let kmh = r.read_u16_le()?;
    let position = r.read_u16_le()?;
    let cup_position = r.read_u16_le()?;
    let track_position = r.read_u16_le()?;
    let spline_position = r.read_f32_le()?;
    let laps = r.read_u16_le()?;
    let delta_ms = r.read_i32_le()?;
    let best_session_lap = parse_lap(r)?;
    let last_lap = parse_lap(r)?;
    let current_lap = parse_lap(r)?;
    Some(RealtimeCarUpdate {
        car_index,
        driver_index,
        driver_count,
        gear,
        world_pos_x,
        world_pos_y,
        yaw,
        car_location,
        kmh,
        position,
        cup_position,
        track_position,
        spline_position,
        laps,
        delta_ms,
        best_session_lap,
        last_lap,
        current_lap,
    })
}

fn parse_lap(r: &mut Reader) -> Option<LapInfo> {
    let lap_time_ms = r.read_i32_le()?;
    let car_index = r.read_u16_le()?;
    let driver_index = r.read_u16_le()?;
    let split_count = r.read_u8()? as usize;
    let mut splits_ms = Vec::with_capacity(split_count);
    for _ in 0..split_count {
        splits_ms.push(r.read_i32_le()?);
    }
    let is_invalid = r.read_u8()? > 0;
    let is_valid_for_best = r.read_u8()? > 0;
    let is_outlap = r.read_u8()? > 0;
    let is_inlap = r.read_u8()? > 0;
    Some(LapInfo {
        lap_time_ms,
        car_index,
        driver_index,
        splits_ms,
        is_invalid,
        is_valid_for_best,
        is_outlap,
        is_inlap,
    })
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.buf.len() {
            return None;
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Some(s)
    }
    fn read_u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn read_i8(&mut self) -> Option<i8> {
        Some(self.take(1)?[0] as i8)
    }
    fn read_u16_le(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn read_i32_le(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn read_f32_le(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn read_string(&mut self) -> Option<String> {
        let len = self.read_u16_le()? as usize;
        let bytes = self.take(len)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Decode a `session_type` byte to its name. Values from the ACC SDK.
pub fn session_type_name(t: u8) -> &'static str {
    match t {
        0 => "Practice",
        4 => "Qualifying",
        9 => "Superpole",
        10 => "Race",
        11 => "Hotlap",
        12 => "TimeAttack",
        13 => "HotStint",
        14 => "HotlapSuperpole",
        255 => "None",
        _ => "Unknown",
    }
}

/// Decode a `phase` byte to its name. Values from the ACC SDK.
pub fn session_phase_name(p: u8) -> &'static str {
    match p {
        0 => "None",
        1 => "Starting",
        2 => "PreFormation",
        3 => "FormationLap",
        4 => "PreSession",
        5 => "Session",
        6 => "SessionOver",
        7 => "PostSession",
        8 => "ResultUI",
        _ => "Unknown",
    }
}

/// Decode a `car_location` byte. Values from the ACC SDK.
pub fn car_location_name(loc: u8) -> &'static str {
    match loc {
        0 => "None",
        1 => "Track",
        2 => "Pitlane",
        3 => "PitEntry",
        4 => "PitExit",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_packet_layout() {
        let pkt = build_register("hello", "secret", 250, "");
        // [type=1][ver=2][len=5]hello[len=6]secret[i32 250 LE][len=0]
        assert_eq!(pkt[0], 1);
        assert_eq!(pkt[1], PROTOCOL_VERSION);
        assert_eq!(&pkt[2..4], &5u16.to_le_bytes());
        assert_eq!(&pkt[4..9], b"hello");
        assert_eq!(&pkt[9..11], &6u16.to_le_bytes());
        assert_eq!(&pkt[11..17], b"secret");
        assert_eq!(&pkt[17..21], &250i32.to_le_bytes());
        assert_eq!(&pkt[21..23], &0u16.to_le_bytes());
        assert_eq!(pkt.len(), 23);
    }

    #[test]
    fn unregister_packet_layout() {
        let pkt = build_unregister(0x01020304);
        assert_eq!(pkt, [9, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn registration_result_decodes() {
        // [type=1][i32 conn=42][success=1][readonly_byte=0=>readonly=true][len=0 errmsg]
        let mut buf = vec![inbound::REGISTRATION_RESULT];
        buf.extend_from_slice(&42i32.to_le_bytes());
        buf.push(1);
        buf.push(0);
        buf.extend_from_slice(&0u16.to_le_bytes());
        match parse_message(&buf).unwrap() {
            Message::RegistrationResult(r) => {
                assert_eq!(r.connection_id, 42);
                assert!(r.success);
                assert!(r.readonly);
                assert_eq!(r.error_message, "");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn lap_format_handles_sentinel() {
        let lap = LapInfo {
            lap_time_ms: i32::MAX,
            car_index: 0,
            driver_index: 0,
            splits_ms: vec![],
            is_invalid: false,
            is_valid_for_best: true,
            is_outlap: false,
            is_inlap: false,
        };
        assert_eq!(lap.format(), "--:--.---");
        let lap2 = LapInfo {
            lap_time_ms: 102_345,
            ..lap
        };
        assert_eq!(lap2.format(), "01:42.345");
    }

    #[test]
    fn gear_label_handles_acc_offsets() {
        let mut c = sample_car_update();
        c.gear = -2;
        assert_eq!(c.gear_label(), "?");
        c.gear = -1;
        assert_eq!(c.gear_label(), "R");
        c.gear = 0;
        assert_eq!(c.gear_label(), "N");
        c.gear = 4;
        assert_eq!(c.gear_label(), "4");
    }

    fn sample_car_update() -> RealtimeCarUpdate {
        RealtimeCarUpdate {
            car_index: 0,
            driver_index: 0,
            driver_count: 1,
            gear: 0,
            world_pos_x: 0.0,
            world_pos_y: 0.0,
            yaw: 0.0,
            car_location: 0,
            kmh: 0,
            position: 0,
            cup_position: 0,
            track_position: 0,
            spline_position: 0.0,
            laps: 0,
            delta_ms: 0,
            best_session_lap: empty_lap(),
            last_lap: empty_lap(),
            current_lap: empty_lap(),
        }
    }

    fn empty_lap() -> LapInfo {
        LapInfo {
            lap_time_ms: i32::MAX,
            car_index: 0,
            driver_index: 0,
            splits_ms: vec![],
            is_invalid: false,
            is_valid_for_best: true,
            is_outlap: false,
            is_inlap: false,
        }
    }
}
