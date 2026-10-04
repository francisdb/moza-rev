// Moza Racing serial protocol — minimal client for driving the wheel's RPM LED bar.
//
// Frame:  7E [N] [group] [device] [cmd_id...] [payload...] [chk]
//   N    = cmd_id.len() + payload.len()
//   chk  = (0x0D + sum(all preceding bytes) + 0x7E * count_of_0x7E_in_body[2..]) & 0xFF
//
// Body 0x7E bytes get doubled on the wire (byte stuffing). RPM bitmask body
// can't contain 0x7E in practice (highest bit-set bytes 0xFF/0x03 etc.), but
// the checksum itself can equal 0x7E and must be doubled too.
//
// Two LED telemetry commands depending on wheel generation:
//
// Modern wheels (CS Pro / KS Pro / FSR / R9 / R12 / R16 / R21 + matching wheels):
//   group 0x3F (wheel write), device 0x17 (wheel), cmd [0x1A, 0x00],
//   payload = LE bitmask of lit LEDs.
//   ≤16 LEDs:  u16 LE  → frame: 7E 04 3F 17 1A 00 b0 b1 chk
//   >16 LEDs:  u32 LE  → frame: 7E 06 3F 17 1A 00 b0 b1 b2 b3 chk
//
// ES (legacy) wheels (R3 / R5 base + bundled ES wheel):
//   group 0x41 (base write telemetry), device 0x13 (base), cmd [0xFD, 0xDE],
//   payload = 4-byte big-endian bitmask. Frame: 7E 06 41 13 FD DE b3 b2 b1 b0 chk
//   The base proxies the bitmask to the wheel internally — frames are
//   addressed to the BASE (0x13), NOT the wheel (0x17). Sending to 0x17 fills
//   some firmware queue and eventually locks the base requiring a power cycle.
//   "RPM Indicator Mode" setup (group 0x40, device 0x13, cmd [0x04], value 1):
//   1=telemetry-driven, 2=off, 3=always-on. Default is often 3 (all LEDs lit),
//   so we write 1 on connect or our bitmask writes are ignored.
//
// Source: docs/moza-protocol.md and Devices/MozaLedDeviceManager.cs in
// giantorth/moza-simhub-plugin (and cross-checked against Lawstorant/boxflat).

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MESSAGE_START: u8 = 0x7E;
const CHECKSUM_MAGIC: u8 = 0x0D;
pub const GROUP_WHEEL_READ: u8 = 0x40;
pub const GROUP_WHEEL_WRITE: u8 = 0x3F;
const GROUP_BASE_TELEMETRY: u8 = 0x41;
/// Read-only group for the base's runtime telemetry (temps, state). Boxflat
/// catalogs these as `read: 43` (= 0x2B) with `write: -1`.
const GROUP_BASE_TELEM_READ: u8 = 0x2B;
const BAUD_RATE: u32 = 115200;

/// Wheel device id (decimal 23 in boxflat's `serial.yml`). Used for the
/// Modern protocol (group 0x3F, cmd 1A 00) which addresses the wheel directly.
pub const DEVICE_WHEEL: u8 = 0x17;

/// Base device id (decimal 19 in boxflat's `serial.yml`). Used for the Legacy
/// protocol on R3/R5: LED telemetry frames and the indicator-mode setup are
/// addressed to the base, which proxies them to the attached ES wheel.
pub const DEVICE_BASE: u8 = 0x13;

/// "RPM Indicator Mode" setting cmd. Group 0x40 (settings write) on the base.
/// 1 = driven by telemetry frames, 2 = always off, 3 = always on.
const GROUP_BASE_SETTING_WRITE: u8 = 0x40;
const ES_INDICATOR_MODE_TELEMETRY: u8 = 1;

/// Wheelbase runtime temperatures (degrees Celsius). Each is `None` if the
/// underlying read timed out — sensors are independent so partial results
/// are common when the bus is busy.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaseTemps {
    pub mcu_c: Option<f32>,
    pub mosfet_c: Option<f32>,
    pub motor_c: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Newer wheels: cmd 0x1A 0x00, group 0x3F, LE bitmask.
    Modern,
    /// R3/R5 base ES wheels: cmd 0xFD 0xDE, group 0x41, BE 4-byte bitmask.
    Legacy,
}

/// Pick a protocol from the /dev/serial/by-id name. R3/R5 bases use the
/// legacy ES protocol; everything else (R9, R12, R16, R21, CS Pro, KS Pro,
/// FSR, …) uses the modern protocol.
pub fn detect_protocol(serial_path: &str) -> Protocol {
    let lower = serial_path.to_lowercase();
    if lower.contains("_r5_") || lower.contains("_r3_") {
        Protocol::Legacy
    } else {
        Protocol::Modern
    }
}

pub struct Moza {
    port: Box<dyn serialport::SerialPort>,
    protocol: Protocol,
    initialized_led_count: Option<usize>,
}

impl Moza {
    /// The device id frames should be addressed to. Modern wheels: 0x17 (the
    /// wheel itself). Legacy R3/R5: 0x13 (the base, which proxies to the wheel).
    fn device_id(&self) -> u8 {
        match self.protocol {
            Protocol::Modern => DEVICE_WHEEL,
            Protocol::Legacy => DEVICE_BASE,
        }
    }
}

impl Moza {
    pub fn open(path: &str, protocol: Protocol) -> io::Result<Self> {
        // Match pyserial's default: 8N1, no flow control, non-exclusive
        // (boxflat opens with `exclusive=False`). serialport-rs defaults to
        // exclusive=true with TIOCEXCL + flock, which the Moza firmware
        // may treat differently from pyserial's permissive open.
        let port = serialport::new(path, BAUD_RATE)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(500))
            .exclusive(false)
            .open()
            .map_err(|e| io::Error::other(format!("open {path}: {e}")))?;
        let mut moza = Self {
            port,
            protocol,
            initialized_led_count: None,
        };
        let _ = moza.port.clear(serialport::ClearBuffer::All);

        if protocol == Protocol::Legacy {
            // Switch the LED bar from "always-on" to telemetry-driven. Boxflat
            // does this via group 0x40 / device 0x13 / cmd 0x04 / value 1.
            moza.write_frame(&build_frame(
                GROUP_BASE_SETTING_WRITE,
                DEVICE_BASE,
                &[0x04],
                &[ES_INDICATOR_MODE_TELEMETRY],
            ))?;
        }
        Ok(moza)
    }

    /// Read MCU / MOSFET / motor temperatures from the wheelbase, in degrees
    /// Celsius. Each value is `None` if its individual read timed out within
    /// `per_read_deadline`. The wire format is centidegrees (e.g. 0x0E10 =
    /// 3600 = 36.00 °C).
    pub fn read_base_temps(&mut self, per_read_deadline: Duration) -> io::Result<BaseTemps> {
        Ok(BaseTemps {
            mcu_c: self.read_centidegrees(0x04, per_read_deadline)?,
            mosfet_c: self.read_centidegrees(0x05, per_read_deadline)?,
            motor_c: self.read_centidegrees(0x06, per_read_deadline)?,
        })
    }

    fn read_centidegrees(&mut self, cmd: u8, deadline: Duration) -> io::Result<Option<f32>> {
        // Match boxflat's exact wire bytes: `7e:03:2b:13:cmd:00:01:chk`. The
        // trailing `01` byte is what the firmware expects as a "read 1 value"
        // marker; sending all-zeros there gets ignored.
        self.write_frame(&build_frame(
            GROUP_BASE_TELEM_READ,
            DEVICE_BASE,
            &[cmd],
            &[0x00, 0x01],
        ))?;
        let want_group = GROUP_BASE_TELEM_READ | 0x80;
        let start = Instant::now();
        while start.elapsed() < deadline {
            let remaining = deadline.saturating_sub(start.elapsed());
            let Some(frame) = self.read_frame(remaining)? else {
                return Ok(None);
            };
            if frame.group != want_group || frame.payload.len() < 3 || frame.payload[0] != cmd {
                continue;
            }
            let raw = u16::from_be_bytes([frame.payload[1], frame.payload[2]]);
            return Ok(Some(raw as f32 / 100.0));
        }
        Ok(None)
    }

    pub fn send_rpm_bitmask(&mut self, bitmask: u32, led_count: usize) -> io::Result<()> {
        if !(1..=32).contains(&led_count) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "LED count must be between 1 and 32",
            ));
        }
        let dev = self.device_id();
        if self.protocol == Protocol::Modern && self.initialized_led_count != Some(led_count) {
            // The bitmask alone is insufficient: modern wheels also need
            // external telemetry mode and a non-black telemetry colour table.
            // These are the same commands used by Boxflat's RPM test. The
            // telemetry palette is separate from the wheel's static colours.
            for frame in build_modern_rpm_setup_frames(dev, led_count) {
                self.write_frame(&frame)?;
            }
            self.initialized_led_count = Some(led_count);
        }
        let frame = match self.protocol {
            Protocol::Modern => build_modern_rpm_frame(dev, bitmask, led_count),
            Protocol::Legacy => build_legacy_rpm_frame(dev, bitmask),
        };
        self.write_frame(&frame)
    }

    /// Send an RPM percent (0..=1023) for ES (legacy) wheels. The wheel
    /// firmware compares this against its threshold table (read via
    /// `rpm-timings` cmd 0x02 — defaults around 75..94% in Percent mode) and
    /// lights LEDs accordingly. Use this on legacy wheels when driven by
    /// real telemetry — `send_rpm_bitmask` is for modern wheels where the
    /// payload is interpreted as a literal LED bitmask.
    pub fn send_rpm_percent_1023(&mut self, value: u16) -> io::Result<()> {
        let value = value.min(1023) as u32;
        let frame = build_legacy_rpm_frame(self.device_id(), value);
        self.write_frame(&frame)
    }

    /// Borrow a clone of the underlying serial port for read-only listening
    /// from another thread. Both handles share the same kernel file
    /// descriptor.
    pub fn try_clone_port(&self) -> io::Result<Box<dyn serialport::SerialPort>> {
        self.port
            .try_clone()
            .map_err(|e| io::Error::other(format!("try_clone: {e}")))
    }

    fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let stuffed = byte_stuff(frame);
        log::trace!("→ {}", hex(&stuffed));
        self.port.write_all(&stuffed)?;
        self.port.flush()
    }

    /// Read a setting via request/response. Sends a read frame for
    /// `(read_group, DEVICE_WHEEL, cmd_id)`; waits up to `deadline` for any
    /// response whose group matches `read_group | 0x80` and whose payload
    /// starts with `cmd_id`. We deliberately do NOT filter by device ID:
    /// ES wheels on R3/R5 bases reply tagged with the base's device id 0x13
    /// (= 0x31 nibble-swapped) instead of the wheel's 0x17, because the
    /// wheel isn't its own USB endpoint and the base proxies responses.
    pub fn read_setting(
        &mut self,
        read_group: u8,
        cmd_id: &[u8],
        response_payload_len: usize,
        deadline: Duration,
    ) -> io::Result<Option<Vec<u8>>> {
        // Boxflat reads settings with an integer value of 1, encoded in
        // big-endian form across the setting's payload width. Zero-valued
        // requests are ignored by some firmware.
        let mut request_payload = vec![0u8; response_payload_len];
        if let Some(last) = request_payload.last_mut() {
            *last = 1;
        }
        let dev = self.device_id();
        self.write_frame(&build_frame(read_group, dev, cmd_id, &request_payload))?;
        let want_group = read_group | 0x80;

        let start = Instant::now();
        while start.elapsed() < deadline {
            let remaining = deadline.saturating_sub(start.elapsed());
            let Some(frame) = self.read_frame(remaining)? else {
                return Ok(None);
            };
            if frame.group != want_group {
                continue;
            }
            if frame.payload.len() < cmd_id.len() {
                continue;
            }
            if &frame.payload[..cmd_id.len()] != cmd_id {
                continue;
            }
            return Ok(Some(frame.payload[cmd_id.len()..].to_vec()));
        }
        Ok(None)
    }

    /// Scan for a checksum-validated frame within `deadline`. Skips text
    /// debug logs and any frame whose checksum doesn't verify.
    fn read_frame(&mut self, deadline: Duration) -> io::Result<Option<Frame>> {
        let start = Instant::now();
        let mut byte = [0u8; 1];

        loop {
            if start.elapsed() >= deadline {
                return Ok(None);
            }
            // Find a 0x7E start byte.
            loop {
                if start.elapsed() >= deadline {
                    return Ok(None);
                }
                match self.port.read(&mut byte) {
                    Ok(1) if byte[0] == MESSAGE_START => break,
                    Ok(_) => continue,
                    Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                    Err(e) => return Err(e),
                }
            }
            // Length.
            if self.port.read(&mut byte)? != 1 {
                continue;
            }
            let n = byte[0] as usize;
            if !(1..=64).contains(&n) {
                continue;
            }
            // group + device + N payload + checksum.
            let mut rest = vec![0u8; n + 3];
            if self.port.read_exact(&mut rest).is_err() {
                continue;
            }
            // Verify checksum: sum over [0x7E, n, group, device, payload..],
            // mod 256. (No body bytes equal 0x7E in practice for setting
            // responses, so wire-checksum and raw-checksum agree.)
            let mut sum: u32 = CHECKSUM_MAGIC as u32 + MESSAGE_START as u32 + n as u32;
            for &b in &rest[..rest.len() - 1] {
                sum += b as u32;
            }
            // Account for body 0x7E bytes that would have been doubled on
            // the wire (and counted twice in the wire-level checksum).
            for &b in &rest[..rest.len() - 1] {
                if b == MESSAGE_START {
                    sum += MESSAGE_START as u32;
                }
            }
            let computed = (sum & 0xFF) as u8;
            let actual = *rest.last().unwrap();
            if computed != actual {
                log::trace!(
                    "✗ frame chk mismatch  computed=0x{computed:02X} actual=0x{actual:02X}  raw={}",
                    hex(&rest)
                );
                continue;
            }
            let group = rest[0];
            let device = rest[1];
            let payload = rest[2..rest.len() - 1].to_vec();
            log::trace!(
                "← group=0x{group:02X} device=0x{device:02X} payload={}",
                hex(&payload)
            );
            return Ok(Some(Frame {
                group,
                device,
                payload,
            }));
        }
    }
}

#[derive(Debug)]
pub struct Frame {
    pub group: u8,
    pub device: u8,
    pub payload: Vec<u8>,
}

#[allow(dead_code)]
fn swap_nibbles(b: u8) -> u8 {
    b.rotate_right(4)
}

/// Find the wheelbase serial port via /dev/serial/by-id/. Moza devices have
/// "Gudsen" in the by-id name; the wheelbase is the one whose name contains
/// "Base" (lowercased: "base"). Falls back to first Gudsen entry if no "base"
/// match — handy for hubs/AB9.
pub fn find_wheelbase() -> Option<String> {
    let dir = PathBuf::from("/dev/serial/by-id");
    let entries: Vec<_> = fs::read_dir(&dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains("gudsen"))
                .unwrap_or(false)
        })
        .collect();

    let base = entries.iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_lowercase().contains("base"))
            .unwrap_or(false)
    });
    base.or_else(|| entries.first())
        .and_then(|p| p.to_str().map(String::from))
}

fn build_modern_rpm_frame(device: u8, bitmask: u32, led_count: usize) -> Vec<u8> {
    let payload: Vec<u8> = if led_count > 16 {
        bitmask.to_le_bytes().to_vec()
    } else {
        (bitmask as u16).to_le_bytes().to_vec()
    };
    build_frame(GROUP_WHEEL_WRITE, device, &[0x1A, 0x00], &payload)
}

fn build_modern_rpm_setup_frames(device: u8, led_count: usize) -> Vec<Vec<u8>> {
    let mut frames = vec![build_frame(GROUP_WHEEL_WRITE, device, &[0x1C, 0x00], &[1])];
    // Each colour message contains five [LED index, R, G, B] records.
    // Repeat the final record to fill the last message on bars whose LED
    // count is not a multiple of five, without addressing nonexistent LEDs.
    for start in (0..led_count).step_by(5) {
        let mut payload = Vec::with_capacity(20);
        for offset in 0..5 {
            let index = (start + offset).min(led_count - 1);
            let colour = if index * 10 < led_count * 5 {
                [0, 255, 0]
            } else if index * 10 < led_count * 8 {
                [255, 180, 0]
            } else {
                [255, 0, 0]
            };
            payload.push(index as u8);
            payload.extend_from_slice(&colour);
        }
        frames.push(build_frame(
            GROUP_WHEEL_WRITE,
            device,
            &[0x19, 0x00],
            &payload,
        ));
    }
    frames
}

fn build_legacy_rpm_frame(device: u8, value: u32) -> Vec<u8> {
    // ES wheels: 4-byte big-endian RPM value (0..1023 in Percent mode), not
    // a literal LED bitmask — wheel firmware maps it to LEDs via thresholds.
    let payload = value.to_be_bytes();
    build_frame(GROUP_BASE_TELEMETRY, device, &[0xFD, 0xDE], &payload)
}

fn build_frame(group: u8, device: u8, cmd_id: &[u8], payload: &[u8]) -> Vec<u8> {
    let n = (cmd_id.len() + payload.len()) as u8;
    let mut frame = Vec::with_capacity(5 + n as usize);
    frame.push(MESSAGE_START);
    frame.push(n);
    frame.push(group);
    frame.push(device);
    frame.extend_from_slice(cmd_id);
    frame.extend_from_slice(payload);
    frame.push(wire_checksum(&frame));
    frame
}

fn wire_checksum(decoded_frame: &[u8]) -> u8 {
    let mut sum: u32 = CHECKSUM_MAGIC as u32;
    for &b in decoded_frame {
        sum += b as u32;
    }
    // Each 0x7E in body positions 2.. gets doubled on the wire and the
    // sender includes both copies in the checksum.
    for &b in &decoded_frame[2..] {
        if b == MESSAGE_START {
            sum += MESSAGE_START as u32;
        }
    }
    (sum & 0xFF) as u8
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Byte-stuff the frame for the wire: bytes 0..1 (start, length) are emitted
/// raw; from index 2 onward each 0x7E is doubled. Also doubles the checksum
/// when it equals 0x7E.
fn byte_stuff(frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.len() + 4);
    if frame.len() < 2 {
        return frame.to_vec();
    }
    out.push(frame[0]);
    out.push(frame[1]);
    for &b in &frame[2..] {
        out.push(b);
        if b == MESSAGE_START {
            out.push(MESSAGE_START);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn setting_read_sends_the_firmware_read_marker() {
        use serialport::SerialPort;

        let (mut peer, mut port) = serialport::TTYPort::pair().unwrap();
        peer.set_timeout(Duration::from_secs(1)).unwrap();
        port.set_timeout(Duration::from_secs(1)).unwrap();
        let responder = std::thread::spawn(move || {
            let mut request = [0; 8];
            peer.read_exact(&mut request).unwrap();
            assert_eq!(request, [0x7E, 0x03, 0x40, 0x17, 0x1C, 0x00, 0x01, 0x02]);
            // Telemetry-mode response captured from the R12 + CS V2P.
            peer.write_all(&[0x7E, 0x03, 0xC0, 0x71, 0x1C, 0x00, 0x01, 0xDC])
                .unwrap();
            // Keep the peer open until the reply has been consumed.
            peer
        });
        let mut wheel = Moza {
            port: Box::new(port),
            protocol: Protocol::Modern,
            initialized_led_count: None,
        };
        let reply = wheel
            .read_setting(GROUP_WHEEL_READ, &[0x1C, 0x00], 1, Duration::from_secs(1))
            .unwrap();
        let _peer = responder.join().unwrap();
        assert_eq!(reply, Some(vec![1]));
    }

    #[test]
    fn modern_setup_enables_telemetry_and_loads_all_ten_colours() {
        let frames = build_modern_rpm_setup_frames(DEVICE_WHEEL, 10);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0], [0x7E, 0x03, 0x3F, 0x17, 0x1C, 0x00, 0x01, 0x01]);
        for (batch, frame) in frames[1..].iter().enumerate() {
            assert_eq!(&frame[..6], &[0x7E, 0x16, 0x3F, 0x17, 0x19, 0x00]);
            assert_eq!(frame.len(), 27);
            for (offset, record) in frame[6..26].as_chunks::<4>().0.iter().enumerate() {
                assert_eq!(record[0] as usize, batch * 5 + offset);
                assert!(record[1..].iter().any(|&v| v != 0));
            }
        }
        assert_eq!(&frames[1][6..10], &[0, 0, 255, 0]);
        assert_eq!(&frames[2][6..10], &[5, 255, 180, 0]);
        assert_eq!(&frames[2][22..26], &[9, 255, 0, 0]);
    }

    #[test]
    fn modern_setup_pads_partial_colour_batch_with_last_led() {
        let frames = build_modern_rpm_setup_frames(DEVICE_WHEEL, 18);
        assert_eq!(frames.len(), 5);
        let last = &frames[4];
        assert_eq!(&last[14..18], &[17, 255, 0, 0]);
        assert_eq!(&last[18..22], &last[14..18]);
        assert_eq!(&last[22..26], &last[14..18]);
    }

    #[test]
    fn modern_rpm_frame_no_leds_lit() {
        let frame = build_modern_rpm_frame(DEVICE_WHEEL, 0, 10);
        assert_eq!(
            frame,
            vec![0x7E, 0x04, 0x3F, 0x17, 0x1A, 0x00, 0x00, 0x00, 0xFF]
        );
    }

    #[test]
    fn modern_rpm_frame_all_10_lit() {
        let frame = build_modern_rpm_frame(DEVICE_WHEEL, 0x03FF, 10);
        assert_eq!(
            frame,
            vec![0x7E, 0x04, 0x3F, 0x17, 0x1A, 0x00, 0xFF, 0x03, 0x01]
        );
    }

    #[test]
    fn modern_rpm_frame_18_leds_uses_4_byte_payload() {
        let frame = build_modern_rpm_frame(DEVICE_WHEEL, 0x0003_FFFF, 18);
        assert_eq!(&frame[..6], &[0x7E, 0x06, 0x3F, 0x17, 0x1A, 0x00]);
        assert_eq!(&frame[6..10], &[0xFF, 0xFF, 0x03, 0x00]);
    }

    #[test]
    fn legacy_rpm_frame_full() {
        // Matches boxflat wire trace: `7e:06:41:13:fd:de:00:00:03:ff:c2`.
        let frame = build_legacy_rpm_frame(DEVICE_BASE, 0x03FF);
        assert_eq!(
            frame,
            vec![
                0x7E, 0x06, 0x41, 0x13, 0xFD, 0xDE, 0x00, 0x00, 0x03, 0xFF, 0xC2
            ]
        );
    }

    #[test]
    fn legacy_rpm_frame_zero() {
        // Matches boxflat wire trace: `7e:06:41:13:fd:de:00:00:00:00:c0`.
        let frame = build_legacy_rpm_frame(DEVICE_BASE, 0);
        assert_eq!(
            frame,
            vec![
                0x7E, 0x06, 0x41, 0x13, 0xFD, 0xDE, 0x00, 0x00, 0x00, 0x00, 0xC0
            ]
        );
    }

    #[test]
    fn detect_legacy_for_r5() {
        let p = "/dev/serial/by-id/usb-Gudsen_MOZA_R5_Base_480023000F51333135363734-if00";
        assert_eq!(detect_protocol(p), Protocol::Legacy);
    }

    #[test]
    fn detect_modern_for_r9() {
        let p = "/dev/serial/by-id/usb-Gudsen_MOZA_R9_Base_xxx-if00";
        assert_eq!(detect_protocol(p), Protocol::Modern);
    }
}
