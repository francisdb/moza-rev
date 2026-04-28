# moza-rev

Drive a Moza Racing wheel's RPM LED bar from game telemetry. Linux-native, no SimHub required.

## Quick start

```sh
sudo dnf install systemd-devel        # Fedora — or `apt install libudev-dev` on Debian/Ubuntu
sudo usermod -aG dialout $USER        # serial-port access; re-login after
cargo build --release
cargo run --release -- configure      # detect installed games, offer to enable telemetry
cargo run --release                   # drive the LEDs
```

`configure` scans for installed Steam games and proposes the right per-game telemetry edits. It is read-only by default — every change requires a `y/N` confirmation and writes a `.bak` next to the original file.

## Tested devices

- Moza R5 Bundle (R5 base + bundled ES wheel) — confirmed working.

Other Moza wheelbases should work via the modern protocol path (auto-selected from the USB serial id) but haven't been verified. Reports welcome.

## Supported games

A single `cargo run` binds three UDP listeners simultaneously and routes whichever is active to the LEDs. The status line tag (`[WF2]`, `[DR2]`, `[BNG]`) shows the current source; the wheel reverts to its idle breathing pattern after 2 s without telemetry.

| Game | UDP port | `configure` | Notes |
|------|----------|-------------|-------|
| Wreckfest 2 | 23123 | ✓ auto | Bugbear "Pino" format |
| DiRT Rally 2.0 | 20777 | ✓ auto | Codemasters EGO "extradata" format |
| DiRT Showdown | 20777 | ✓ auto | Same legacy format |
| Other Codemasters EGO titles ¹ | 20777 | manual | Same listener handles all |
| BeamNG.drive | 4444 | ✓ auto | LFS-OutGauge format |
| Live For Speed (and other OutGauge clients) | 4444 | manual | Same listener |
| Wreckfest 1, DIRT 5 | — | flagged | No native UDP — would need [SpaceMonkey](https://github.com/PHARTGAMES/SpaceMonkey) under Wine |

¹ DiRT 2 / 3, F1 2010-2017, GRID / GRID 2 / GRID Autosport, DiRT Rally 1.0 all share the same UDP format on port 20777 and should work, but haven't been individually verified.

## Setup details

`moza-rev configure` is the recommended path — it walks all the games above, shows current telemetry state, and offers to apply the right edits with confirmation and a backup. Manual instructions are below for reference.

### Wreckfest 2

Edit `Documents/My Games/Wreckfest 2/<userid>/savegame/telemetry/config.json` and set `"enabled": 1` in the `udp` array. Default port `23123`.

### DiRT Rally 2.0 / DiRT Showdown / older Codemasters EGO games

Edit `Documents/My Games/<game>/hardwaresettings/hardware_settings_config.xml`. Find the `<udp>` element (newer games) or `<motion>` element (Showdown / older games) and set `enabled="true"`, `extradata="3"`, `ip="127.0.0.1"`. Default port `20777`.

### BeamNG.drive

In game: `Options → Other → Protocols → OutGauge`: tick the checkbox, set IP `127.0.0.1`, port `4444`. (Or use `configure`, which edits the underlying `BeamNG.drive/current/settings/cloud/settings.json` directly.)

Caveat: OutGauge doesn't transmit a redline RPM. moza-rev adaptively tracks the highest RPM observed per session and uses that as an effective redline (initial assumption: 7000 RPM, idle: 800 RPM). The bar self-tunes after a few revs.

## Prerequisites

- Rust (stable, 2024 edition).
- `libudev` development headers — `sudo dnf install systemd-devel` on Fedora, `sudo apt install libudev-dev` on Debian/Ubuntu.
- Read/write access to the Moza serial device. The wheelbase shows up as `/dev/ttyACM*` and `/dev/serial/by-id/usb-Gudsen_MOZA_*`. On permission-denied, add yourself to the `dialout` group (or `uucp` on Arch) and re-login: `sudo usermod -aG dialout $USER`.

## Running

```sh
cargo run --release
```

Useful flags:

```sh
--wf2-port    23123      # change Wreckfest 2 UDP port
--dr2-port    20777      # change Codemasters legacy UDP port
--beamng-port 4444       # change BeamNG OutGauge UDP port
--serial /dev/ttyACM0    # override the autodetected serial path
--leds 10                # number of LEDs on the wheel
--protocol legacy        # force legacy (R3/R5/ES); modern is default
--verbose                # log every UDP packet's RPM
```

## Diagnostic examples

```sh
cargo run --example led_chase    # visual chase pattern; confirms LED writes work
cargo run --example blast 1023   # full bar at sustained rate; sanity-check the wire
cargo run --example logcat       # dump every Moza protocol frame on the bus
cargo run --example diagnose     # probe wheel settings (RPM mode, indicator mode, etc.)
cargo run --example wf2_log      # one-line summary per Wreckfest 2 telemetry packet
cargo run --example dr2_log      # one-line summary per Codemasters EGO packet
cargo run --example beamng_log   # one-line summary per OutGauge packet
cargo run --example ams2_log     # one-line summary per AMS2 / PC2 telemetry packet
```

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
- [PHARTGAMES/SpaceMonkey](https://github.com/PHARTGAMES/SpaceMonkey) — multi-game telemetry tool that supports games without native UDP output (including Wreckfest 1 and DIRT 5) by reading game memory. Windows-only and uses memory injection, so it's the obvious bridge if you want LEDs in a game that doesn't expose telemetry over UDP.
- [PHARTGAMES/WreckfestSimFeedback](https://github.com/PHARTGAMES/WreckfestSimFeedback) — SimFeedback motion-telemetry provider for Wreckfest.
