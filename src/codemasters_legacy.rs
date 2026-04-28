// Codemasters "extradata" UDP telemetry parser.
//
// One wire format covers most pre-F1-2018 Codemasters EGO-engine titles:
// DiRT 2 (2009), DiRT 3 (2011), DiRT Showdown (2012), F1 2010-2017,
// GRID / GRID 2 / GRID Autosport, DiRT Rally (2015), DiRT Rally 2.0 (2019).
// The format is a contiguous little-endian array of f32 fields — no header,
// no signature, no version byte. Packet length is selected via the
// `extradata` setting in `hardware_settings_config.xml`:
//
//   extradata=0   ->   64 bytes (basic motion-platform data)
//   extradata=1   ->  152 bytes (adds wheel speeds, positions, etc.)
//   extradata=2   ->  ~244 bytes
//   extradata=3   ->  264 bytes (all 66 fields, the format we mirror)
//
// Schema cross-referenced from
// https://github.com/ErlerPhilipp/dr2_logger/blob/master/source/dirt_rally/udp_data.py
//
// Wire encoding caveat: the rpm / max_rpm / idle_rpm fields are stored as
// `actual RPM / 10`. The struct names them `*_div_10` and the [`rpm`],
// [`redline_rpm`], [`idle_rpm`] accessor methods do the multiply.

use std::ptr;

/// extradata=3 packet: 66 contiguous little-endian f32 fields, 264 bytes.
/// All fields are `f32` because the wire format is float-only — even the
/// integer-by-nature ones (gear, current_lap, total_laps, in_pit) are
/// transmitted as float values.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Telemetry {
    pub run_time: f32,
    pub lap_time: f32,
    pub distance: f32,
    /// 0..=1 fraction of lap completed.
    pub progress: f32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    /// Speed in m/s. Use [`Telemetry::speed_kmh`] for km/h.
    pub speed_ms: f32,
    pub vel_x: f32,
    pub vel_y: f32,
    pub vel_z: f32,
    pub roll_x: f32,
    pub roll_y: f32,
    pub roll_z: f32,
    pub pitch_x: f32,
    pub pitch_y: f32,
    pub pitch_z: f32,
    pub susp_rl: f32,
    pub susp_rr: f32,
    pub susp_fl: f32,
    pub susp_fr: f32,
    pub susp_vel_rl: f32,
    pub susp_vel_rr: f32,
    pub susp_vel_fl: f32,
    pub susp_vel_fr: f32,
    pub wsp_rl: f32,
    pub wsp_rr: f32,
    pub wsp_fl: f32,
    pub wsp_fr: f32,
    /// 0..=1
    pub throttle: f32,
    /// -1..=+1
    pub steering: f32,
    /// 0..=1
    pub brake: f32,
    /// 0..=1
    pub clutch: f32,
    /// Float-encoded integer. Use [`Telemetry::gear_label`] for a string.
    pub gear: f32,
    pub g_force_lat: f32,
    pub g_force_lon: f32,
    pub current_lap: f32,
    /// Wire value: `actual_rpm / 10`. Use [`Telemetry::rpm`].
    pub rpm_div_10: f32,
    pub sli_pro_support: f32,
    pub car_pos: f32,
    pub kers_level: f32,
    pub kers_max_level: f32,
    pub drs: f32,
    pub traction_control: f32,
    pub anti_lock_brakes: f32,
    pub fuel_in_tank: f32,
    pub fuel_capacity: f32,
    pub in_pit: f32,
    pub sector: f32,
    pub sector_1_time: f32,
    pub sector_2_time: f32,
    pub brakes_temp_rl: f32,
    pub brakes_temp_rr: f32,
    pub brakes_temp_fl: f32,
    pub brakes_temp_fr: f32,
    pub tyre_pressure_rl: f32,
    pub tyre_pressure_rr: f32,
    pub tyre_pressure_fl: f32,
    pub tyre_pressure_fr: f32,
    pub laps_completed: f32,
    pub total_laps: f32,
    pub track_length: f32,
    pub last_lap_time: f32,
    /// Wire value: `redline_rpm / 10`. Use [`Telemetry::redline_rpm`].
    pub max_rpm_div_10: f32,
    /// Wire value: `idle_rpm / 10`. Use [`Telemetry::idle_rpm`].
    pub idle_rpm_div_10: f32,
    pub max_gears: f32,
}

/// Size of an extradata=3 packet: 264 bytes.
pub const PACKET_BYTES: usize = std::mem::size_of::<Telemetry>();
/// Number of f32 fields in an extradata=3 packet: 66.
pub const FIELD_COUNT: usize = 66;
/// Codemasters' canonical UDP port (configurable in
/// `hardware_settings_config.xml`).
pub const DEFAULT_PORT: u16 = 20777;

impl Telemetry {
    /// Parse a full extradata=3 packet (264 bytes). Returns `None` if the
    /// buffer is too short. For lower extradata levels use
    /// [`Telemetry::from_bytes_partial`].
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < PACKET_BYTES {
            return None;
        }
        // SAFETY: buf has at least PACKET_BYTES bytes and Telemetry is a
        // POD (repr(C), all f32 fields, every bit pattern valid).
        Some(unsafe { ptr::read_unaligned(buf.as_ptr() as *const Self) })
    }

    /// Parse however many fields fit in `buf`. Bytes beyond the buffer are
    /// zeroed in the result, so e.g. an extradata=0 packet leaves
    /// `rpm_div_10` and the rest zeroed.
    pub fn from_bytes_partial(buf: &[u8]) -> Self {
        let mut out = Self::default();
        let copy_len = buf.len().min(PACKET_BYTES);
        // SAFETY: dst points to a fully-initialized `Self` (zeroed via
        // Default), and copy_len is bounded by both src and dst length.
        unsafe {
            ptr::copy_nonoverlapping(
                buf.as_ptr(),
                std::ptr::from_mut(&mut out).cast::<u8>(),
                copy_len,
            );
        }
        out
    }

    /// Actual engine RPM (wire value × 10).
    pub fn rpm(&self) -> i32 {
        (self.rpm_div_10 * 10.0) as i32
    }

    /// Redline RPM (the upshift threshold).
    pub fn redline_rpm(&self) -> i32 {
        (self.max_rpm_div_10 * 10.0) as i32
    }

    /// Idle RPM.
    pub fn idle_rpm(&self) -> i32 {
        (self.idle_rpm_div_10 * 10.0) as i32
    }

    /// Speed in km/h. Wire value is m/s.
    pub fn speed_kmh(&self) -> f32 {
        self.speed_ms * 3.6
    }

    /// Display label for current gear: "R", "N", or the gear number as a
    /// string. Codemasters' convention varies a bit across titles, so
    /// negative values are treated as Reverse, 0 as Neutral.
    pub fn gear_label(&self) -> String {
        let g = self.gear as i32;
        if g < 0 {
            "R".to_string()
        } else if g == 0 {
            "N".to_string()
        } else {
            g.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn struct_size_is_264_bytes() {
        assert_eq!(PACKET_BYTES, 264);
        assert_eq!(PACKET_BYTES, FIELD_COUNT * 4);
    }

    #[test]
    fn rpm_fields_are_at_documented_offsets() {
        // Cross-checked against ErlerPhilipp/dr2_logger field indices.
        assert_eq!(offset_of!(Telemetry, rpm_div_10), 37 * 4);
        assert_eq!(offset_of!(Telemetry, max_rpm_div_10), 63 * 4);
        assert_eq!(offset_of!(Telemetry, idle_rpm_div_10), 64 * 4);
        assert_eq!(offset_of!(Telemetry, speed_ms), 7 * 4);
        assert_eq!(offset_of!(Telemetry, throttle), 29 * 4);
        assert_eq!(offset_of!(Telemetry, gear), 33 * 4);
    }

    #[test]
    fn from_bytes_rejects_short_buffer() {
        let buf = [0u8; PACKET_BYTES - 1];
        assert!(Telemetry::from_bytes(&buf).is_none());
    }

    #[test]
    fn from_bytes_decodes_full_packet() {
        let mut buf = vec![0u8; PACKET_BYTES];
        // Write 700.0 at the rpm_div_10 offset → rpm() should return 7000.
        let rpm_off = 37 * 4;
        buf[rpm_off..rpm_off + 4].copy_from_slice(&700.0f32.to_le_bytes());
        let max_off = 63 * 4;
        buf[max_off..max_off + 4].copy_from_slice(&800.0f32.to_le_bytes());
        let idle_off = 64 * 4;
        buf[idle_off..idle_off + 4].copy_from_slice(&90.0f32.to_le_bytes());

        let t = Telemetry::from_bytes(&buf).unwrap();
        assert_eq!(t.rpm(), 7000);
        assert_eq!(t.redline_rpm(), 8000);
        assert_eq!(t.idle_rpm(), 900);
    }

    #[test]
    fn from_bytes_partial_zero_fills_missing_fields() {
        // extradata=0 sends ~64 bytes; rpm_div_10 lives at offset 148 so
        // it should come out zero.
        let buf = vec![0xABu8; 64];
        let t = Telemetry::from_bytes_partial(&buf);
        assert_eq!({ t.rpm_div_10 }, 0.0);
        assert_eq!(t.rpm(), 0);
        // First few fields should reflect the input bytes.
        assert_ne!({ t.run_time }, 0.0);
    }

    #[test]
    fn gear_label_handles_reverse_neutral_forward() {
        let r = Telemetry {
            gear: -1.0,
            ..Default::default()
        };
        assert_eq!(r.gear_label(), "R");
        let n = Telemetry::default();
        assert_eq!(n.gear_label(), "N");
        let third = Telemetry {
            gear: 3.0,
            ..Default::default()
        };
        assert_eq!(third.gear_label(), "3");
    }

    #[test]
    fn speed_kmh_converts_from_ms() {
        let t = Telemetry {
            speed_ms: 10.0,
            ..Default::default()
        };
        assert!((t.speed_kmh() - 36.0).abs() < 1e-4);
    }
}
