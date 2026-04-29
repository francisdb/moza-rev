// Forza "Data Out" UDP telemetry — shared wire format used by Forza
// Motorsport 7, Forza Horizon 4, Forza Horizon 5, and (with extensions)
// Forza Motorsport 2023. Two sizes:
//
// - Sled    (232 bytes) — original motion-rig format, FM7 only
// - Dash    (311 / 324 / 331 bytes) — Sled + lap/position/etc, varies
//                                     by title. FH5 sends 324.
//
// All variants share the same first 20 bytes. We only decode that
// prefix here because it carries everything moza-rev needs to drive
// the LED bar — `engine_max_rpm` (redline) and `engine_idle_rpm` are
// transmitted by the game, so no adaptive-redline tracking is needed.
//
// Wire format: little-endian, `#pragma pack(1)`. The game sends one
// packet ~60 Hz to whatever IP:port is configured in
// `Settings → HUD and Gameplay → Data Out`. There is no handshake;
// just bind a UDP socket on the configured port.
//
// Reference: austinbaccus/forza-telemetry, Forza forums "Data Out
// Telemetry Variables and Structure" thread.

use std::ptr;

/// No game-imposed default — the user picks any free port in the
/// in-game data-out menu. We pick 9999 (a common community choice) so
/// `--forza-port` can be omitted in the typical setup.
pub const DEFAULT_PORT: u16 = 9999;

/// Smallest known Forza packet (FM7 Sled).
pub const MIN_PACKET_BYTES: usize = 232;

/// Bytes of the shared header, all variants. Reading just this gives
/// us race-on / RPM / redline / idle.
pub const HEADER_BYTES: usize = std::mem::size_of::<Header>();

/// First 20 bytes of every Forza Data Out packet. Stable across Sled,
/// Dash V1 (FM7 / FH4 / FH5), and Dash V2 (FM-2023).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Header {
    /// 0 = paused / menu / not driving, 1 = on track. Matches LEDs-off vs LEDs-on.
    pub is_race_on: u32,
    /// Game-side monotonic timestamp in milliseconds.
    pub timestamp_ms: u32,
    pub engine_max_rpm: f32,
    pub engine_idle_rpm: f32,
    pub current_engine_rpm: f32,
}

impl Header {
    /// Decode the header from an incoming UDP datagram. Rejects packets
    /// smaller than `MIN_PACKET_BYTES` to avoid acting on garbage from
    /// some other process that happens to share the port.
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < MIN_PACKET_BYTES {
            return None;
        }
        // SAFETY: Header is repr(C, packed), all-POD, and the buffer is
        // at least MIN_PACKET_BYTES (≥ HEADER_BYTES).
        Some(unsafe { ptr::read_unaligned(buf.as_ptr().cast()) })
    }

    pub fn is_race_on(&self) -> bool {
        let v = { self.is_race_on };
        v != 0
    }

    pub fn rpm(&self) -> i32 {
        ({ self.current_engine_rpm }) as i32
    }

    pub fn redline_rpm(&self) -> i32 {
        ({ self.engine_max_rpm }) as i32
    }

    pub fn idle_rpm(&self) -> i32 {
        ({ self.engine_idle_rpm }) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_20_bytes() {
        // 4 (race) + 4 (ts) + 4 (max) + 4 (idle) + 4 (current).
        assert_eq!(HEADER_BYTES, 20);
    }

    #[test]
    fn rejects_packets_below_sled_size() {
        // 231 bytes is short of even the Sled length.
        let short = vec![0u8; 231];
        assert!(Header::from_bytes(&short).is_none());
    }

    #[test]
    fn accepts_minimum_sled_size() {
        let mut buf = vec![0u8; MIN_PACKET_BYTES];
        // is_race_on=1, ts=12345, max=7800, idle=900, current=4500.
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[4..8].copy_from_slice(&12345u32.to_le_bytes());
        buf[8..12].copy_from_slice(&7800f32.to_le_bytes());
        buf[12..16].copy_from_slice(&900f32.to_le_bytes());
        buf[16..20].copy_from_slice(&4500f32.to_le_bytes());
        let h = Header::from_bytes(&buf).expect("decode");
        assert!(h.is_race_on());
        assert_eq!(h.rpm(), 4500);
        assert_eq!(h.redline_rpm(), 7800);
        assert_eq!(h.idle_rpm(), 900);
    }

    #[test]
    fn accepts_fh5_dash_size() {
        // Larger packets must still decode the shared header cleanly.
        let mut buf = vec![0u8; 324];
        buf[0..4].copy_from_slice(&0u32.to_le_bytes());
        buf[8..12].copy_from_slice(&8500f32.to_le_bytes());
        buf[12..16].copy_from_slice(&850f32.to_le_bytes());
        buf[16..20].copy_from_slice(&3200f32.to_le_bytes());
        let h = Header::from_bytes(&buf).expect("decode");
        assert!(!h.is_race_on());
        assert_eq!(h.redline_rpm(), 8500);
        assert_eq!(h.idle_rpm(), 850);
        assert_eq!(h.rpm(), 3200);
    }
}
