// Madness-engine UDP telemetry parser. Same wire format used by Project
// CARS 1, 2, 3 and Automobilista 2 — Reiza built AMS2 on the SMS Madness
// engine and reuses the PC2 telemetry SDK wholesale.
//
// All datagrams arrive on a single UDP port (default 5606) and start with
// a 12-byte header whose `packet_type` byte (offset 10) identifies which
// of the eight packet bodies follows. We mirror the C++ structs from
// MacManley/project-cars-2-udp field-for-field via `#[repr(C, packed)]`
// (the C++ uses `#pragma pack(1)`).
//
// NOTE on Linux: AMS2 broadcasts to `255.255.255.255` and the kernel does
// not loop limited-broadcast traffic back to local sockets. To consume
// telemetry on the same machine you need an iptables NAT redirect. See
// the README's "Automobilista 2 / Project CARS 2" subsection.

use std::ptr;

pub const DEFAULT_PORT: u16 = 5606;

pub const PACKET_TYPE_TELEMETRY: u8 = 0;
pub const PACKET_TYPE_RACE: u8 = 1;
pub const PACKET_TYPE_PARTICIPANTS: u8 = 2;
pub const PACKET_TYPE_TIMINGS: u8 = 3;
pub const PACKET_TYPE_GAME_STATE: u8 = 4;
pub const PACKET_TYPE_WEATHER: u8 = 5;
pub const PACKET_TYPE_VEHICLE_CLASS_NAMES: u8 = 6;
pub const PACKET_TYPE_TIME_STATS: u8 = 7;
pub const PACKET_TYPE_PARTICIPANT_VEHICLE_NAMES: u8 = 8;

pub const HEADER_BYTES: usize = std::mem::size_of::<Header>();
pub const TELEMETRY_PACKET_BYTES: usize = std::mem::size_of::<TelemetryPacket>();

pub fn packet_type_name(t: u8) -> &'static str {
    match t {
        0 => "Telemetry",
        1 => "Race",
        2 => "Participants",
        3 => "Timings",
        4 => "GameState",
        5 => "Weather",
        6 => "VehicleClassNames",
        7 => "TimeStats",
        8 => "ParticipantVehicleNames",
        _ => "Unknown",
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub packet_number: u32,
    pub category_packet_number: u32,
    pub partial_packet_index: u8,
    pub partial_packet_number: u8,
    pub packet_type: u8,
    pub packet_version: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TelemetryData {
    pub viewed_participant_index: i8,
    pub unfiltered_throttle: u8,
    pub unfiltered_brake: u8,
    pub unfiltered_steering: i8,
    pub unfiltered_clutch: u8,
    pub car_flags: u8,
    pub oil_temp_celsius: i16,
    pub oil_pressure_kpa: u16,
    pub water_temp_celsius: i16,
    pub water_pressure_kpa: i16,
    pub fuel_pressure_kpa: u16,
    pub fuel_capacity: u8,
    pub brake: u8,
    pub throttle: u8,
    pub clutch: u8,
    pub fuel_level: f32,
    pub speed: f32,
    pub rpm: u16,
    pub max_rpm: u16,
    pub steering: i8,
    /// Low nibble: gear (0=N, 1..n=forward, 15=R). High nibble: total gears.
    pub gear_num_gears: u8,
    pub boost_amount: u8,
    pub crash_state: u8,
    pub odometer_km: f32,
    pub orientation: [f32; 3],
    pub local_velocity: [f32; 3],
    pub world_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub local_acceleration: [f32; 3],
    pub world_acceleration: [f32; 3],
    pub extents_centre: [f32; 3],
    pub tyre_flags: [u8; 4],
    pub terrains: [u8; 4],
    pub tyre_y: [f32; 4],
    pub tyre_rps: [f32; 4],
    pub tyre_temp: [u8; 4],
    pub tyre_height_above_ground: [f32; 4],
    pub tyre_wear: [u8; 4],
    pub brake_damage: [u8; 4],
    pub suspension_damage: [u8; 4],
    pub brake_temp_celsius: [i16; 4],
    pub tyre_tread_temp: [u16; 4],
    pub tyre_layer_temp: [u16; 4],
    pub tyre_carcass_temp: [u16; 4],
    pub tyre_rim_temp: [u16; 4],
    pub tyre_internal_air_temp: [u16; 4],
    pub tyre_temp_left: [u16; 4],
    pub tyre_temp_center: [u16; 4],
    pub tyre_temp_right: [u16; 4],
    pub wheel_local_position_y: [f32; 4],
    pub ride_height: [f32; 4],
    pub suspension_travel: [f32; 4],
    pub suspension_velocity: [f32; 4],
    pub suspension_ride_height: [u16; 4],
    pub air_pressure: [u16; 4],
    pub engine_speed: f32,
    pub engine_torque: f32,
    pub wings: [u8; 2],
    pub hand_brake: u8,
    pub aero_damage: u8,
    pub engine_damage: u8,
    pub joy_pad: u32,
    pub d_pad: u8,
    pub tyre_compound: [[u8; 40]; 4],
    pub turbo_boost_pressure: f32,
    pub full_position: [f32; 3],
    pub brake_bias: u8,
    pub tick_count: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TelemetryPacket {
    pub header: Header,
    pub data: TelemetryData,
}

impl Header {
    /// Parse the 12-byte header off the front of any Madness-engine
    /// datagram. Returns `None` if the buffer is shorter than the header.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_BYTES {
            return None;
        }
        // SAFETY: Header is repr(C, packed) and buf has at least HEADER_BYTES.
        Some(unsafe { ptr::read_unaligned(buf.as_ptr().cast()) })
    }
}

impl TelemetryPacket {
    /// Parse a full telemetry datagram (559 bytes). Returns `None` if the
    /// buffer is too short OR the header's packet type isn't `Telemetry`.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < TELEMETRY_PACKET_BYTES {
            return None;
        }
        let header = Header::from_bytes(buf)?;
        if header.packet_type != PACKET_TYPE_TELEMETRY {
            return None;
        }
        // SAFETY: TelemetryPacket is repr(C, packed) and buf has at
        // least TELEMETRY_PACKET_BYTES.
        Some(unsafe { ptr::read_unaligned(buf.as_ptr().cast()) })
    }
}

impl TelemetryData {
    // Accessors return values by Copy. The `let` bindings make sure we
    // never take a reference into the packed struct (which would be UB).
    pub fn rpm(&self) -> i32 {
        let v = self.rpm;
        i32::from(v)
    }
    pub fn redline_rpm(&self) -> i32 {
        let v = self.max_rpm;
        i32::from(v)
    }
    /// Speed in km/h. Wire value is m/s.
    pub fn speed_kmh(&self) -> f32 {
        let v = self.speed;
        v * 3.6
    }
    /// Current gear, 0 = N, 1..=n = forward gears, 15 = R.
    pub fn gear(&self) -> u8 {
        self.gear_num_gears & 0x0F
    }
    /// Total gears the car has.
    pub fn num_gears(&self) -> u8 {
        (self.gear_num_gears >> 4) & 0x0F
    }
    /// Display label: "R", "N", "1", "2", …
    pub fn gear_label(&self) -> String {
        match self.gear() {
            0 => "N".to_string(),
            15 => "R".to_string(),
            n => n.to_string(),
        }
    }
    /// Filtered throttle as a fraction (0..=1). Wire value is 0..=255.
    pub fn throttle_frac(&self) -> f32 {
        f32::from(self.throttle) / 255.0
    }
    pub fn brake_frac(&self) -> f32 {
        f32::from(self.brake) / 255.0
    }
    pub fn clutch_frac(&self) -> f32 {
        f32::from(self.clutch) / 255.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_size_matches_pcars2_pbase() {
        assert_eq!(HEADER_BYTES, 12);
    }

    #[test]
    fn telemetry_packet_size_matches_pcars2_layout() {
        // Cross-checked against MacManley/project-cars-2-udp:
        // header (12) + TelemetryData (547) = 559.
        assert_eq!(TELEMETRY_PACKET_BYTES, 559);
    }

    #[test]
    fn telemetry_packet_rejects_non_telemetry_types() {
        let mut buf = vec![0u8; TELEMETRY_PACKET_BYTES];
        // packet_type byte is at offset 10 (after two u32 + two u8).
        buf[10] = PACKET_TYPE_RACE;
        assert!(TelemetryPacket::from_bytes(&buf).is_none());
    }

    #[test]
    fn gear_label_decodes_packed_byte() {
        let mut t: TelemetryData = unsafe { std::mem::zeroed() };
        // 7 forward gears, currently in 3rd.
        t.gear_num_gears = (7 << 4) | 3;
        assert_eq!(t.gear(), 3);
        assert_eq!(t.num_gears(), 7);
        assert_eq!(t.gear_label(), "3");
        // Reverse.
        t.gear_num_gears = (7 << 4) | 0x0F;
        assert_eq!(t.gear_label(), "R");
        // Neutral.
        t.gear_num_gears = 7 << 4;
        assert_eq!(t.gear_label(), "N");
    }
}
