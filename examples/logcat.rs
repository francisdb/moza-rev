// Read everything from /dev/ttyACM0 and split into:
//   firmware log frames  (group=0x0E, payload is ASCII)        → "LOG  ..."
//   other protocol frames (validated by checksum)              → "FRAME group=...  N=..  payload=..."
//   stray bytes outside any frame                              → "RAW  ..."
//
// Handles 0x7E byte stuffing per protocol: any 0x7E in the body or checksum
// is doubled on the wire, so after reading a complete frame we consume one
// extra 0x7E for each 0x7E that appeared in the body.
//
// Run:
//   cargo run --example logcat
//   cargo run --example logcat -- --serial /dev/ttyACM0

use std::env;
use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use log::{error, info};

use moza_rev::moza::{self};

const MESSAGE_START: u8 = 0x7E;
const CHECKSUM_MAGIC: u8 = 0x0D;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut serial_override: Option<String> = None;
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        if arg == "--serial" {
            serial_override = it.next();
        }
    }

    let path = match serial_override.or_else(moza::find_wheelbase) {
        Some(p) => p,
        None => {
            error!("no Moza wheelbase found");
            return ExitCode::FAILURE;
        }
    };

    let mut port = match serialport::new(&path, 115200)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(Duration::from_millis(200))
        .exclusive(false)
        .open()
    {
        Ok(p) => p,
        Err(e) => {
            error!("open {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    info!("logcat reading from {path}  (Ctrl+C to stop)");

    let mut byte = [0u8; 1];
    let mut raw = Vec::<u8>::new();

    loop {
        match port.read(&mut byte) {
            Ok(1) => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                flush_raw(&mut raw);
                continue;
            }
            Err(e) => {
                error!("read error: {e}");
                return ExitCode::FAILURE;
            }
        }
        let b = byte[0];
        if b != MESSAGE_START {
            raw.push(b);
            continue;
        }
        flush_raw(&mut raw);

        // Read length, expect 1..=64.
        let mut len_buf = [0u8; 1];
        if read_one(&mut *port, &mut len_buf).is_err() {
            raw.push(b);
            continue;
        }
        let n = len_buf[0] as usize;
        if !(1..=64).contains(&n) {
            raw.push(b);
            raw.push(len_buf[0]);
            continue;
        }

        // Read body+checksum, de-stuffing 0x7E on the fly. Each body 0x7E
        // appears doubled on the wire, so we consume the extra copy.
        let total = n + 3;
        let mut rest = Vec::with_capacity(total);
        let mut bad = false;
        while rest.len() < total {
            let mut b = [0u8; 1];
            if read_one(&mut *port, &mut b).is_err() {
                bad = true;
                break;
            }
            rest.push(b[0]);
            if b[0] == MESSAGE_START {
                // Eat the duplicate.
                let mut dup = [0u8; 1];
                let _ = read_one(&mut *port, &mut dup);
                // If dup isn't 0x7E, we've desync'd; bail.
                if dup[0] != MESSAGE_START {
                    bad = true;
                    break;
                }
            }
        }
        if bad {
            continue;
        }

        // Verify checksum (wire-checksum: include +0x7E for each body 0x7E).
        let mut sum: u32 = CHECKSUM_MAGIC as u32 + MESSAGE_START as u32 + n as u32;
        for &x in &rest[..rest.len() - 1] {
            sum += x as u32;
        }
        for &x in &rest[..rest.len() - 1] {
            if x == MESSAGE_START {
                sum += MESSAGE_START as u32;
            }
        }
        let expected = (sum & 0xFF) as u8;
        let actual = *rest.last().unwrap();
        if expected != actual {
            // Not a real frame — surface the raw bytes for debugging.
            raw.push(b);
            raw.push(len_buf[0]);
            raw.extend_from_slice(&rest);
            continue;
        }

        let group = rest[0];
        let device = rest[1];
        let payload = &rest[2..rest.len() - 1];

        // Firmware log frames: group=0x0E with payload[1..] containing ASCII
        // text (e.g. "[WARN]serial_cmd...c:NN ..."). The first payload byte is
        // a type/severity marker (often 0x05 for warnings observed). Other
        // group=0x0E frames carry binary status — distinguish by whether the
        // tail is printable.
        if group == 0x0E && payload.len() > 1 && is_mostly_printable(&payload[1..]) {
            let text = ascii_render(&payload[1..]);
            match payload[0] {
                1 | 2 => log::debug!("device=0x{device:02X}  {text}"),
                3 => info!("device=0x{device:02X}  {text}"),
                4 | 5 => log::warn!("device=0x{device:02X}  {text}"),
                6 => error!("device=0x{device:02X}  {text}"),
                _ => info!("device=0x{device:02X}  {text}"),
            }
            continue;
        }

        info!(
            "FRAME  group=0x{group:02X}  device=0x{device:02X}  N={n:>2}  payload={}",
            hex(payload)
        );
    }
}

fn read_one(port: &mut dyn serialport::SerialPort, buf: &mut [u8; 1]) -> std::io::Result<()> {
    loop {
        match port.read(buf) {
            Ok(1) => return Ok(()),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => return Err(e),
        }
    }
}

fn is_mostly_printable(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let printable = bytes
        .iter()
        .filter(|&&b| (0x20..=0x7E).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t')
        .count();
    // Threshold: at least 80% printable AND at least 4 chars total. Filters
    // out short binary payloads like "\x00\x15\x00\x01".
    bytes.len() >= 4 && printable * 5 >= bytes.len() * 4
}

fn ascii_render(bytes: &[u8]) -> String {
    // Try UTF-8 first so multi-byte chars like ° (C2 B0) render correctly.
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.replace(['\n', '\r'], "");
    }
    // Fall back to byte-by-byte if invalid UTF-8.
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\n' | b'\r' => {}
            0x20..=0x7E => s.push(b as char),
            _ => s.push_str(&format!("\\x{:02X}", b)),
        }
    }
    s
}

fn flush_raw(buf: &mut Vec<u8>) {
    if buf.is_empty() {
        return;
    }
    info!("RAW    {}", ascii_render(buf));
    buf.clear();
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
