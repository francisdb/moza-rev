// Spam the wheel with a fixed RPM value AND concurrently dump every frame
// + firmware log line we receive. Single process, two threads sharing the
// same serial fd via SerialPort::try_clone().
//
// Run:
//   cargo run --example blast -- 1023        # full bar
//   cargo run --example blast -- 0           # all off
//   cargo run --example blast -- 800 --hz 60 # custom rate

use std::env;
use std::io::Read;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, error, info};

use moza_rev::moza::{self, Moza, Protocol};

const MESSAGE_START: u8 = 0x7E;
const CHECKSUM_MAGIC: u8 = 0x0D;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut value: u16 = 1023;
    let mut hz: u32 = 60;
    let mut secs: u64 = 5;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hz" => hz = args.next().and_then(|s| s.parse().ok()).unwrap_or(60),
            "--secs" => secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(5),
            other => {
                if let Ok(v) = other.parse::<u16>() {
                    value = v.min(1023);
                }
            }
        }
    }

    let serial_path = match moza::find_wheelbase() {
        Some(p) => p,
        None => {
            error!("no wheelbase");
            return ExitCode::FAILURE;
        }
    };
    let protocol = moza::detect_protocol(&serial_path);
    info!("opening {serial_path} ({protocol:?})");
    let mut wheel = match Moza::open(&serial_path, protocol) {
        Ok(w) => w,
        Err(e) => {
            error!("open: {e}");
            return ExitCode::FAILURE;
        }
    };

    let read_port = match wheel.try_clone_port() {
        Ok(p) => p,
        Err(e) => {
            error!("try_clone: {e}");
            return ExitCode::FAILURE;
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || reader_loop(read_port, stop))
    };

    info!(
        "blasting value {value} ({:.1}%) at ~{hz}Hz for {secs}s",
        value as f32 * 100.0 / 1023.0
    );
    let interval = Duration::from_secs_f64(1.0 / hz as f64);
    let start = Instant::now();
    let mut writes = 0u32;
    while start.elapsed() < Duration::from_secs(secs) {
        let res = match protocol {
            Protocol::Legacy => wheel.send_rpm_percent_1023(value),
            Protocol::Modern => wheel.send_rpm_bitmask(value as u32, 10),
        };
        if let Err(e) = res {
            error!("write: {e}");
            break;
        }
        writes += 1;
        thread::sleep(interval);
    }

    stop.store(true, Ordering::SeqCst);
    info!("sent {writes} frames; flushing reader for 500ms...");
    thread::sleep(Duration::from_millis(500));
    let _ = reader.join();
    ExitCode::SUCCESS
}

fn reader_loop(mut port: Box<dyn serialport::SerialPort>, stop: Arc<AtomicBool>) {
    // Use a short timeout so we wake up periodically and check `stop`.
    let _ = port.set_timeout(Duration::from_millis(100));

    let mut byte = [0u8; 1];
    let mut raw = Vec::<u8>::new();

    while !stop.load(Ordering::SeqCst) {
        match port.read(&mut byte) {
            Ok(1) => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                flush_raw(&mut raw);
                continue;
            }
            Err(e) => {
                error!("read err: {e}");
                return;
            }
        }
        let b = byte[0];
        if b != MESSAGE_START {
            raw.push(b);
            continue;
        }
        flush_raw(&mut raw);

        let mut len = [0u8; 1];
        if read_one(&mut port, &mut len).is_err() {
            raw.push(b);
            continue;
        }
        let n = len[0] as usize;
        if !(1..=64).contains(&n) {
            raw.push(b);
            raw.push(len[0]);
            continue;
        }

        let total = n + 3;
        let mut rest = Vec::with_capacity(total);
        let mut bad = false;
        while rest.len() < total {
            let mut bb = [0u8; 1];
            if read_one(&mut port, &mut bb).is_err() {
                bad = true;
                break;
            }
            rest.push(bb[0]);
            if bb[0] == MESSAGE_START {
                let mut dup = [0u8; 1];
                let _ = read_one(&mut port, &mut dup);
                if dup[0] != MESSAGE_START {
                    bad = true;
                    break;
                }
            }
        }
        if bad {
            continue;
        }

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
            raw.push(b);
            raw.push(len[0]);
            raw.extend_from_slice(&rest);
            continue;
        }

        let group = rest[0];
        let device = rest[1];
        let payload = &rest[2..rest.len() - 1];

        if group == 0x0E && payload.len() > 1 && is_mostly_printable(&payload[1..]) {
            let text = utf8_render(&payload[1..]);
            match payload[0] {
                1 | 2 => log::debug!("device=0x{device:02X}  {text}"),
                3 => log::info!("device=0x{device:02X}  {text}"),
                4 | 5 => log::warn!("device=0x{device:02X}  {text}"),
                6 => log::error!("device=0x{device:02X}  {text}"),
                _ => log::info!("device=0x{device:02X}  {text}"),
            }
            continue;
        }

        debug!(
            "FRAME  group=0x{group:02X}  device=0x{device:02X}  N={n:>2}  payload={}",
            hex(payload)
        );
    }
    flush_raw(&mut raw);
}

fn read_one(port: &mut Box<dyn serialport::SerialPort>, buf: &mut [u8; 1]) -> std::io::Result<()> {
    loop {
        match port.read(buf) {
            Ok(1) => return Ok(()),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) => return Err(e),
        }
    }
}

fn flush_raw(buf: &mut Vec<u8>) {
    if buf.is_empty() {
        return;
    }
    debug!("RAW    {}", utf8_render(buf));
    buf.clear();
}

fn utf8_render(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.replace(['\n', '\r'], "");
    }
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

fn is_mostly_printable(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let printable = bytes
        .iter()
        .filter(|&&b| (0x20..=0x7E).contains(&b) || matches!(b, b'\n' | b'\r' | b'\t'))
        .count();
    bytes.len() >= 4 && printable * 5 >= bytes.len() * 4
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
