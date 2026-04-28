// LFS-compatible OutGauge UDP telemetry parser.
//
// One wire format covers Live For Speed and BeamNG.drive (and a handful
// of other titles that advertise OutGauge compat). The packet is a
// 96-byte C struct with naturally-aligned fields, so plain `#[repr(C)]`
// maps cleanly without `packed`. The trailing `id` field is optional —
// older 92-byte packets are read with the trailing 4 bytes zeroed.
//
// Schema source for BeamNG:
//   lua/vehicle/protocols/outgauge.lua in the BeamNG.drive install.
// Original LFS docs: docs/InSim.txt (search for "OutGauge").
//
// Notable absences: OutGauge does NOT include max_rpm / redline_rpm or
// idle_rpm. The DL_SHIFT bit in `show_lights` is a binary "shift now"
// hint set by the simulator near redline. Consumers driving an LED bar
// from this need to either supply their own redline guess or
// adaptively track the highest RPM seen in the session.

use std::ptr;

/// Default UDP port both BeamNG and LFS use out of the box.
pub const DEFAULT_PORT: u16 = 4444;

/// Size of an OutGauge packet with the optional `id` trailer (BeamNG default).
pub const PACKET_BYTES: usize = std::mem::size_of::<Packet>();
/// Size without the optional `id` trailer (classic LFS).
pub const MIN_PACKET_BYTES: usize = 92;

/// `flags` field bits.
pub mod og {
    pub const TURBO: u16 = 8192;
    /// User prefers KM/h over MPH (display hint only).
    pub const KM: u16 = 16384;
    /// User prefers BAR over PSI (display hint only).
    pub const BAR: u16 = 32768;
}

/// `dash_lights` / `show_lights` bits. `dash_lights` is the set of
/// indicators this car *has*; `show_lights` is the subset currently lit.
pub mod dl {
    pub const SHIFT: u32 = 1 << 0;
    pub const FULLBEAM: u32 = 1 << 1;
    pub const HANDBRAKE: u32 = 1 << 2;
    pub const PITSPEED: u32 = 1 << 3;
    pub const TC: u32 = 1 << 4;
    pub const SIGNAL_L: u32 = 1 << 5;
    pub const SIGNAL_R: u32 = 1 << 6;
    pub const OILWARN: u32 = 1 << 8;
    pub const BATTERY: u32 = 1 << 9;
    pub const ABS: u32 = 1 << 10;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Packet {
    /// Milliseconds since simulator start. BeamNG hardcodes 0.
    pub time: u32,
    /// Car identifier. BeamNG sends "beam".
    pub car: [u8; 4],
    /// `og::*` flags.
    pub flags: u16,
    /// 0 = R, 1 = N, 2 = 1st, 3 = 2nd, ...
    pub gear: i8,
    /// Player id (BeamNG hardcodes 0).
    pub plid: i8,
    pub speed: f32, // m/s
    pub rpm: f32,
    pub turbo: f32,        // bar
    pub eng_temp: f32,     // °C
    pub fuel: f32,         // 0..=1
    pub oil_pressure: f32, // bar (BeamNG hardcodes 0)
    pub oil_temp: f32,     // °C
    /// `dl::*` mask of lights this car has.
    pub dash_lights: u32,
    /// `dl::*` mask of lights currently lit.
    pub show_lights: u32,
    pub throttle: f32, // 0..=1
    pub brake: f32,    // 0..=1
    pub clutch: f32,   // 0..=1
    pub display1: [u8; 16],
    pub display2: [u8; 16],
    pub id: i32,
}

impl Packet {
    /// Parse an OutGauge UDP datagram. Accepts both the 92-byte LFS
    /// classic and 96-byte BeamNG variants; the missing `id` is read as 0.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < MIN_PACKET_BYTES {
            return None;
        }
        let mut padded = [0u8; PACKET_BYTES];
        let copy_len = buf.len().min(PACKET_BYTES);
        padded[..copy_len].copy_from_slice(&buf[..copy_len]);
        // SAFETY: Packet is repr(C) with all-POD fields, every bit
        // pattern valid; padded is exactly PACKET_BYTES.
        Some(unsafe { ptr::read_unaligned(padded.as_ptr().cast()) })
    }

    /// Trimmed `car` field as a string (drops trailing zero padding).
    pub fn car_name(&self) -> String {
        let trimmed: Vec<u8> = self.car.iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8(trimmed).unwrap_or_else(|_| "?".to_string())
    }

    /// Display label: "R", "N", "1", "2", …
    pub fn gear_label(&self) -> String {
        match self.gear {
            0 => "R".to_string(),
            1 => "N".to_string(),
            n if n > 1 => (n - 1).to_string(),
            n => format!("{n}"),
        }
    }

    pub fn speed_kmh(&self) -> f32 {
        self.speed * 3.6
    }

    /// True when the simulator's "shift now" indicator is lit. Useful as
    /// a binary saturate-the-bar signal when no real redline is known.
    pub fn shift_active(&self) -> bool {
        self.show_lights & dl::SHIFT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_size_is_96_bytes() {
        assert_eq!(PACKET_BYTES, 96);
    }

    #[test]
    fn from_bytes_rejects_short_buffer() {
        let buf = [0u8; 50];
        assert!(Packet::from_bytes(&buf).is_none());
    }

    #[test]
    fn from_bytes_zero_pads_lfs_classic() {
        // 92-byte packet (LFS without id trailer): id should be 0.
        let buf = [0u8; 92];
        let p = Packet::from_bytes(&buf).unwrap();
        assert_eq!({ p.id }, 0);
    }

    #[test]
    fn car_name_strips_trailing_nulls() {
        let p = Packet {
            time: 0,
            car: *b"beam",
            flags: 0,
            gear: 0,
            plid: 0,
            speed: 0.0,
            rpm: 0.0,
            turbo: 0.0,
            eng_temp: 0.0,
            fuel: 0.0,
            oil_pressure: 0.0,
            oil_temp: 0.0,
            dash_lights: 0,
            show_lights: 0,
            throttle: 0.0,
            brake: 0.0,
            clutch: 0.0,
            display1: [0; 16],
            display2: [0; 16],
            id: 0,
        };
        assert_eq!(p.car_name(), "beam");
    }

    #[test]
    fn gear_labels_use_lfs_convention() {
        let mut p = Packet::from_bytes(&[0u8; 96]).unwrap();
        p.gear = 0;
        assert_eq!(p.gear_label(), "R");
        p.gear = 1;
        assert_eq!(p.gear_label(), "N");
        p.gear = 2;
        assert_eq!(p.gear_label(), "1");
        p.gear = 7;
        assert_eq!(p.gear_label(), "6");
    }

    #[test]
    fn shift_active_reads_dl_shift_bit() {
        let mut p = Packet::from_bytes(&[0u8; 96]).unwrap();
        p.show_lights = dl::SHIFT;
        assert!(p.shift_active());
        p.show_lights = dl::HANDBRAKE;
        assert!(!p.shift_active());
    }
}
