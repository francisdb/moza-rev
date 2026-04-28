# moza-rev
Control the moza race wheel rev meter with game telemetry

## Usage

### Prerequisites

- Rust (stable, 2024 edition).
- `libudev` development headers (`sudo dnf install systemd-devel` on Fedora, `sudo apt install libudev-dev` on Debian/Ubuntu).
- Read/write access to the Moza serial device. On most distros the wheelbase shows up under `/dev/ttyACM*` and `/dev/serial/by-id/usb-Gudsen_MOZA_*`. If you get a permission denied opening it, add yourself to the `dialout` group (or `uucp` on Arch) and re-login: `sudo usermod -aG dialout $USER`.

### Build

```sh
cargo build --release
```

### Configure

Scan for installed Steam games and offer to enable their telemetry config (read-only by default — every change requires `y/N` confirmation and writes a `.bak` next to the original):

```sh
cargo run --release -- configure
```

Currently handles: Wreckfest 2 (JSON), DiRT Rally 2.0 + DiRT Showdown (XML, same format). Detects but doesn't auto-edit: BeamNG.drive (prints OutGauge instructions). Detects but flags as no-telemetry: Wreckfest 1, DIRT 5 (these need memory-injection tools like SpaceMonkey).

### Run

Default: listen for Wreckfest 2 (UDP `:23123`) **and** DiRT Rally 2.0 / other Codemasters EGO-engine titles (UDP `:20777`) simultaneously, autodetect the Moza wheelbase, drive the LEDs from whichever game is currently sending packets. The active game is shown in the status line; the wheel falls back to its idle breathing pattern after 2 seconds without any telemetry.

```sh
cargo run --release
```

Useful flags:

```sh
cargo run --release -- --wf2-port 23123      # change Wreckfest 2 UDP port
cargo run --release -- --dr2-port 20777      # change Codemasters legacy UDP port
cargo run --release -- --serial /dev/ttyACM0 # override the autodetected serial path
cargo run --release -- --leds 10             # number of LEDs on the wheel (default 10)
cargo run --release -- --protocol legacy     # force legacy (R3/R5/ES); modern is default for everything else
cargo run --release -- --verbose             # log every UDP packet's RPM
```

### Diagnostic examples

```sh
cargo run --example led_chase    # visual chase pattern; confirms LED writes work
cargo run --example blast 1023   # full bar at sustained rate; sanity-check the wire
cargo run --example logcat       # dump every protocol frame seen on the bus
cargo run --example diagnose     # probe wheel settings (RPM mode, indicator mode, etc.)
cargo run --example wf2_log      # log a one-line summary per Wreckfest 2 telemetry packet
cargo run --example dr2_log      # log a one-line summary per DiRT Rally 2.0 telemetry packet
```

## Tested devices

- Moza R5 Bundle (R5 base + bundled ES wheel) — confirmed working

Other Moza wheelbases should work via the modern protocol path (auto-selected from the USB serial id), but haven't been verified. Reports welcome.

## Supported games

### Wreckfest 2

Telemetry is off by default. Edit the game's `config.json` and set `"enabled": 1`. On Steam (Flatpak) the file lives at:

```
~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/compatdata/1203190/pfx/drive_c/users/steamuser/Documents/My Games/Wreckfest 2/<userid>/savegame/telemetry/config.json
```

For native Steam installs the prefix replaces `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` with `~/.steam/steam`.

Default UDP port is `23123` — matches `moza-rev`'s default. Override with `--wf2-port` if needed.

### DiRT Rally 2.0

Telemetry is off by default. Edit `hardware_settings_config.xml` and flip the `<udp>` element's `enabled` to `"true"` and `extradata` to `"3"`. On Steam (Flatpak) the file lives at:

```
~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/compatdata/690790/pfx/drive_c/users/steamuser/Documents/My Games/DiRT Rally 2.0/hardwaresettings/hardware_settings_config.xml
```

For native Steam installs the prefix replaces `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` with `~/.steam/steam`.

Find the `<motion_platform>` block and set:

```xml
<udp enabled="true" extradata="3" ip="127.0.0.1" port="20777" delay="1" />
```

`extradata=3` is what gives you the full 264-byte packet with RPM, redline, and idle RPM. Lower values send a shorter packet and `dr2_log` will warn that fields are missing. Default UDP port is `20777` — Codemasters' standard. Override with `--dr2-port` if needed.

The same parser handles **any Codemasters EGO-engine title using the legacy "extradata" UDP format**: DiRT 2 / 3 / Showdown, F1 2010-2017, GRID / GRID 2 / GRID Autosport, DiRT Rally, DiRT Rally 2.0. Just point the game at port 20777 and it will be picked up automatically.

## Logging

Uses `env_logger`. Default level is `info`. Override with `RUST_LOG`:

```sh
RUST_LOG=warn   cargo run               # quieter
RUST_LOG=debug  cargo run               # show FRAME/RAW dumps in blast/logcat readers
RUST_LOG=trace  cargo run               # also show every protocol byte sent/received
RUST_LOG=moza_rev=trace cargo run       # trace only the moza-rev crate, leave deps quiet
```

`logcat` dumps every received frame at `info` (its job is to surface them); other binaries keep per-frame output at `debug`.

## Related projects

- [Lawstorant/boxflat](https://github.com/Lawstorant/boxflat) — Moza wheel/base/pedal control GUI for Linux. Reference implementation for the Moza serial protocol; this project's wire format and addressing were validated by capturing and matching boxflat's output byte-for-byte.
- [PHARTGAMES/SpaceMonkey](https://github.com/PHARTGAMES/SpaceMonkey) — multi-game telemetry tool that supports games without native UDP output (including Wreckfest 1) by reading game memory. Windows-only and uses memory injection, so it's the obvious bridge if you want LEDs in a game that doesn't expose telemetry over UDP.
- [PHARTGAMES/WreckfestSimFeedback](https://github.com/PHARTGAMES/WreckfestSimFeedback) — SimFeedback motion-telemetry provider for Wreckfest, also from PHARTGAMES.
