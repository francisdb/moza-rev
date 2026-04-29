// Assetto Corsa 1 "ac remote telemetry" UDP protocol.
//
// Unlike the other listeners in this crate, AC1 doesn't broadcast. The
// client opens an ephemeral UDP socket, sends a Handshake to AC's
// listener port (default 9996), receives a HandshakeResponse identifying
// car + track, then sends a Subscribe — UPDATE for per-physics-step
// RTCarInfo packets (~333 Hz), or SPOT for one packet per second. AC
// replies to whatever address the client sent from, so simply binding
// and `connect()`-ing to the AC port is enough.
//
// Protocol reference: the AC SDK ships `ksdk.h` / `acshared.h` with the
// game install. The wire structs use *default* MSVC packing (4-byte
// alignment), not `#pragma pack(1)` — so the leading `char identifier`
// in `RTCarInfo` is followed by 3 bytes of padding before the next
// `int`, and the 6 `bool` flags are followed by 2 bytes of padding
// before the next `float`. We mirror that with plain `#[repr(C)]`,
// which gives us the same natural alignment as the C struct.
//
// Wide strings in HandshakeResponse are 50-wchar UTF-16 LE buffers
// (Windows wchar_t). AC pads unused tail bytes with '%' rather than NUL,
// so the decoder strips both.

use std::ptr;

/// Default UDP port AC's telemetry listener binds.
pub const DEFAULT_PORT: u16 = 9996;

/// Wire value for the `identifier` field of a Handshake. AC ignores it
/// but the SDK example sends 1.
pub const HANDSHAKE_IDENTIFIER: i32 = 1;
/// Protocol version. AC's SDK uses 1.
pub const HANDSHAKE_VERSION: i32 = 1;

/// Handshake `operation_id` values.
pub mod op {
    /// Initial handshake — server replies with `HandshakeResponse`.
    pub const HANDSHAKE: i32 = 0;
    /// Subscribe to per-physics-step RTCarInfo (~333 Hz).
    pub const SUBSCRIBE_UPDATE: i32 = 1;
    /// Subscribe to one RTCarInfo per second.
    pub const SUBSCRIBE_SPOT: i32 = 2;
    /// Unsubscribe.
    pub const DISMISS: i32 = 3;
}

pub const HANDSHAKE_BYTES: usize = std::mem::size_of::<Handshake>();
pub const HANDSHAKE_RESPONSE_BYTES: usize = std::mem::size_of::<HandshakeResponse>();
pub const RT_CAR_INFO_BYTES: usize = std::mem::size_of::<RtCarInfo>();

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub identifier: i32,
    pub version: i32,
    pub operation_id: i32,
}

impl Handshake {
    pub fn new(operation_id: i32) -> Self {
        Self {
            identifier: HANDSHAKE_IDENTIFIER,
            version: HANDSHAKE_VERSION,
            operation_id,
        }
    }

    pub fn to_bytes(&self) -> [u8; HANDSHAKE_BYTES] {
        let mut out = [0u8; HANDSHAKE_BYTES];
        // SAFETY: Handshake is repr(C, packed), all-POD, and `out` is
        // exactly its size.
        unsafe {
            ptr::copy_nonoverlapping(
                (self as *const Self).cast::<u8>(),
                out.as_mut_ptr(),
                HANDSHAKE_BYTES,
            );
        }
        out
    }
}

/// 408-byte response to op::HANDSHAKE. Strings are 50-wchar UTF-16 LE.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HandshakeResponse {
    pub car_name: [u16; 50],
    pub driver_name: [u16; 50],
    pub identifier: i32,
    pub version: i32,
    pub track_name: [u16; 50],
    pub track_config: [u16; 50],
}

impl HandshakeResponse {
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < HANDSHAKE_RESPONSE_BYTES {
            return None;
        }
        // SAFETY: HandshakeResponse is repr(C, packed) with all-POD
        // fields and the buffer is at least its size.
        Some(unsafe { ptr::read_unaligned(buf.as_ptr().cast()) })
    }

    pub fn car(&self) -> String {
        decode_wide_string(&{ self.car_name })
    }
    pub fn driver(&self) -> String {
        decode_wide_string(&{ self.driver_name })
    }
    pub fn track(&self) -> String {
        decode_wide_string(&{ self.track_name })
    }
    pub fn track_config_name(&self) -> String {
        decode_wide_string(&{ self.track_config })
    }
}

/// AC's wide strings: stop at the first NUL or '%' (AC pads tails with '%'
/// instead of zeroes), then UTF-16 LE decode.
fn decode_wide_string(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&u| u == 0 || u == u16::from(b'%'))
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Streamed every physics step (UPDATE) or once per second (SPOT). The
/// `identifier` byte is `b'a'` and `size` is the byte count of this
/// struct as written by AC — useful to detect protocol version skew.
///
/// The C SDK uses default MSVC packing, so there's 3 bytes of pad
/// between `identifier` and `size`, and 2 bytes of pad between the
/// trailing `is_*` flag and `acc_g_vertical`. Plain `#[repr(C)]` here
/// gives us the same layout (Rust matches C's natural alignment).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RtCarInfo {
    pub identifier: u8,
    pub size: i32,
    pub speed_kmh: f32,
    pub speed_mph: f32,
    pub speed_ms: f32,
    pub is_abs_enabled: u8,
    pub is_abs_in_action: u8,
    pub is_tc_in_action: u8,
    pub is_tc_enabled: u8,
    pub is_in_pit: u8,
    pub is_engine_limiter_on: u8,
    pub acc_g_vertical: f32,
    pub acc_g_horizontal: f32,
    pub acc_g_frontal: f32,
    pub lap_time: i32,
    pub last_lap: i32,
    pub best_lap: i32,
    pub lap_count: i32,
    pub gas: f32,
    pub brake: f32,
    pub clutch: f32,
    pub engine_rpm: f32,
    pub steer: f32,
    pub gear: i32,
    pub cg_height: f32,
    pub wheel_angular_speed: [f32; 4],
    pub slip_angle: [f32; 4],
    pub slip_angle_contact_patch: [f32; 4],
    pub slip_ratio: [f32; 4],
    pub tyre_slip: [f32; 4],
    pub nd_slip: [f32; 4],
    pub load: [f32; 4],
    pub dy: [f32; 4],
    pub mz: [f32; 4],
    pub tyre_dirty_level: [f32; 4],
    pub camber_rad: [f32; 4],
    pub tyre_radius: [f32; 4],
    pub tyre_loaded_radius: [f32; 4],
    pub suspension_height: [f32; 4],
    pub car_position_normalized: f32,
    pub car_slope: f32,
    pub car_coordinates: [f32; 3],
}

impl RtCarInfo {
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < RT_CAR_INFO_BYTES {
            return None;
        }
        // SAFETY: RtCarInfo is repr(C, packed), all-POD, every bit
        // pattern valid; buffer is at least its size.
        Some(unsafe { ptr::read_unaligned(buf.as_ptr().cast()) })
    }

    pub fn rpm(&self) -> i32 {
        self.engine_rpm as i32
    }

    /// AC's gear convention: 0=R, 1=N, 2..=n = forward gears.
    pub fn gear_label(&self) -> String {
        let g = { self.gear };
        match g {
            0 => "R".to_string(),
            1 => "N".to_string(),
            n if n > 1 => (n - 1).to_string(),
            n => n.to_string(),
        }
    }

    /// Lap time in seconds (wire format is milliseconds).
    pub fn lap_time_s(&self) -> f32 {
        self.lap_time as f32 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_is_12_bytes() {
        assert_eq!(HANDSHAKE_BYTES, 12);
    }

    #[test]
    fn handshake_response_is_408_bytes() {
        // 4 strings × 50 wchar × 2 bytes + 2 ints × 4 bytes = 408.
        assert_eq!(HANDSHAKE_RESPONSE_BYTES, 408);
    }

    #[test]
    fn rt_car_info_is_328_bytes_with_msvc_alignment() {
        // 1 (id) + 3 pad + 4 (size) + 3*4 (speeds) + 6 (bools)
        // + 2 pad + 3*4 (accG) + 4*4 (lap ints)
        // + 5*4 (gas/brake/clutch/rpm/steer) + 4 (gear) + 4 (cg)
        // + 14*16 (per-wheel arrays) + 4 + 4 + 12 (position) = 328.
        assert_eq!(RT_CAR_INFO_BYTES, 328);
    }

    #[test]
    fn handshake_to_bytes_is_little_endian_ints() {
        let h = Handshake::new(op::SUBSCRIBE_UPDATE);
        let bytes = h.to_bytes();
        assert_eq!(&bytes[0..4], &1i32.to_le_bytes());
        assert_eq!(&bytes[4..8], &1i32.to_le_bytes());
        assert_eq!(&bytes[8..12], &op::SUBSCRIBE_UPDATE.to_le_bytes());
    }

    #[test]
    fn decode_wide_string_strips_percent_padding() {
        // AC pads the wchar tail with '%' instead of NUL.
        let mut units = [u16::from(b'%'); 50];
        let head = "magione".encode_utf16().collect::<Vec<_>>();
        units[..head.len()].copy_from_slice(&head);
        assert_eq!(decode_wide_string(&units), "magione");
    }

    #[test]
    fn decode_wide_string_stops_at_nul() {
        let mut units = [0u16; 10];
        units[0] = u16::from(b'a');
        units[1] = u16::from(b'b');
        // rest zero
        assert_eq!(decode_wide_string(&units), "ab");
    }

    #[test]
    fn rt_car_info_gear_label_uses_ac_convention() {
        let mut p = RtCarInfo::from_bytes(&[0u8; RT_CAR_INFO_BYTES]).unwrap();
        p.gear = 0;
        assert_eq!(p.gear_label(), "R");
        p.gear = 1;
        assert_eq!(p.gear_label(), "N");
        p.gear = 2;
        assert_eq!(p.gear_label(), "1");
        p.gear = 8;
        assert_eq!(p.gear_label(), "7");
    }
}
