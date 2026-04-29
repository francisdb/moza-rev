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

A single `cargo run` opens all per-game telemetry sources simultaneously and routes whichever is active to the LEDs. The status line tag (`[WF2]`, `[DR2]`, `[BNG]`, `[AMS2]`, `[AC]`) shows the current source; the wheel reverts to its idle breathing pattern after 2 s without telemetry.

| Game | UDP port | `configure` | Notes |
|------|----------|-------------|-------|
| Wreckfest 2 | 23123 | ✓ auto | Bugbear "Pino" format |
| DiRT Rally 2.0 | 20777 | ✓ auto | Codemasters EGO "extradata" format |
| DiRT Showdown | 20777 | ✓ auto | Same legacy format |
| Other Codemasters EGO titles ¹ | 20777 | manual | Same listener handles all |
| BeamNG.drive | 4444 | ✓ auto | LFS-OutGauge format |
| Live For Speed (and other OutGauge clients) | 4444 | manual | Same listener |
| Automobilista 2 / Project CARS 2 | 5606 | manual ² | Requires Linux loopback fix ²ʹ |
| Assetto Corsa 1 | 9996 | always on ³ | Handshake-based; adaptive redline |
| Assetto Corsa Competizione | 9000 | ✓ auto | Broadcasting API ⁴ — gear/speed/lap only, **no RPM** |
| Assetto Corsa Rally | — | flagged | UE5; [shared-memory ring buffer only](https://luizzak.itch.io/racing-overlay/devlog/1321475/assetto-corsa-rally-telemetry-support), no UDP — same shape as iRacing on Linux |
| Forza Horizon 5 / Horizon 4 / Motorsport 7 | 9999 ⁵ | manual | "Data Out" UDP (Sled / Dash); RPM + redline + idle in-protocol |
| Wreckfest 1, DIRT 5 | — | flagged | No native UDP — would need [SpaceMonkey](https://github.com/PHARTGAMES/SpaceMonkey) under Wine |

¹ DiRT 2 / 3, F1 2010-2017, GRID / GRID 2 / GRID Autosport, DiRT Rally 1.0 all share the same UDP format on port 20777 and should work, but haven't been individually verified.

² AMS2 stores its UI settings in encrypted `.sav` files that we can't safely edit, so configure-time setup is in-game only.

²ʹ See the **AMS2 / PC2** subsection below for the iptables one-liner needed on Linux.

³ AC's UDP listener is unconditionally on once a session is loaded — no telemetry toggles. `configure` instead offers to write `steam_appid.txt` next to `acs.exe` to fix the standard Proton launcher workaround (see the **Assetto Corsa** subsection).

⁴ ACC's broadcasting API is connection-oriented (register/result handshake, password). It carries gear, speed, lap, position, weather — but not engine RPM, so it can't drive the LED bar on its own. `configure` enables broadcasting in `broadcasting.json` and the `assetto_corsa_competizione_log` example exercises the protocol; an RPM-capable ACC bridge would still need a Windows-side shared-memory reader.

⁵ No game-imposed default; we suggest 9999 to match the same value `--forza-port` defaults to. Pick anything free and just keep the in-game port and `--forza-port` aligned.

## Setup details

`moza-rev configure` is the recommended path — it walks all the games above, shows current telemetry state, and offers to apply the right edits with confirmation and a backup. Manual instructions are below for reference.

### Wreckfest 2

Edit `Documents/My Games/Wreckfest 2/<userid>/savegame/telemetry/config.json` and set `"enabled": 1` in the `udp` array. Default port `23123`.

### DiRT Rally 2.0 / DiRT Showdown / older Codemasters EGO games

Edit `Documents/My Games/<game>/hardwaresettings/hardware_settings_config.xml`. Find the `<udp>` element (newer games) or `<motion>` element (Showdown / older games) and set `enabled="true"`, `extradata="3"`, `ip="127.0.0.1"`. Default port `20777`.

### BeamNG.drive

In game: `Options → Other → Protocols → OutGauge`: tick the checkbox, set IP `127.0.0.1`, port `4444`. (Or use `configure`, which edits the underlying `BeamNG.drive/current/settings/cloud/settings.json` directly.)

Caveat: OutGauge doesn't transmit a redline RPM. moza-rev adaptively tracks the highest RPM observed per session and uses that as an effective redline (initial assumption: 7000 RPM, idle: 800 RPM). The bar self-tunes after a few revs.

### Automobilista 2 / Project CARS 2

In game: `Options → System` — set **UDP Protocol Version: Project CARS 2**, **UDP Frequency: 1+** (lower = higher rate; 5 ≈ 120 Hz is plenty). Restart the game so the setting actually applies. Default port `5606`. Shared Memory mode (a separate setting in the same panel) doesn't need to match — UDP works regardless of what it's set to.

**Linux gotcha — broadcast loopback.** AMS2 sends to the limited-broadcast address `255.255.255.255:5606`. Linux emits this packet out the routing-default interface (e.g. `enp0s…`) but does **not** loop it back to local sockets, so `cargo run --example automobilista_2_log` sees nothing despite `tcpdump -i any` clearly showing traffic on the `Out` direction. Workaround — redirect the broadcast back to localhost:

```sh
sudo iptables -t nat -I OUTPUT -p udp -d 255.255.255.255 --dport 5606 -j DNAT --to-destination 127.0.0.1:5606
```

This is reversible (`-D` instead of `-I` to remove). Side effect: packets stop going out on the LAN — fine for same-machine telemetry, breaks dual-machine setups.

The same workaround applies to any other Madness-engine title (PC2, PC3). Assetto Corsa 1 doesn't need it — its protocol is request/response (handshake to `127.0.0.1:9996`), so the kernel never has to loop a broadcast.

### Assetto Corsa

UDP telemetry on port 9996 is unconditionally on once a session is loaded — there's no in-game toggle. moza-rev sends a Handshake to AC, gets back the car / track / driver, then subscribes to per-physics-step `RTCarInfo` (~333 Hz, forwarded to the LED loop at ~60 Hz). On menu return / quit the stream goes silent and the listener falls back to handshake retries every 3 s, so it doesn't matter whether AC or moza-rev started first.

Caveat — same as BeamNG: AC's `RTCarInfo` doesn't carry a redline, so moza-rev tracks the highest RPM seen in the session and uses that as an effective redline (initial 7000, idle 800). The bar self-tunes after a few revs.

Linux launcher caveat: AC's stock launcher (`AssettoCorsa.exe`, .NET WPF + CEF3) often fails on current Wine/Proton with an assembly-load error. The standard workaround is to add `acs.exe` (the actual game binary, in the install root) as a non-Steam shortcut and force a stable Proton on it. That direct launch then needs a `steam_appid.txt` containing `244210` next to `acs.exe`, or `acs.exe` exits ~2 s after start with no error message. `moza-rev configure` offers to write that file.

### Forza Horizon 5 / 4, Forza Motorsport 7

In game: `Settings → HUD and Gameplay → Data Out: On`, set `IP Address: 127.0.0.1`, `IP Port: 9999` (or whatever you pass to `--forza-port`). FH5 has no format selector — it's locked to **Dash** (324 B); FM7's selector lets you pick Sled or Dash, but moza-rev's parser handles either. Settings persist inside Proton's compatdata in a layout we haven't reverse-engineered, so this is in-game only — `moza-rev configure` detects FH5/FH4 and prints the steps but doesn't auto-apply.

No Linux gotchas — Forza sends to a single host:port (not broadcast), so the AMS2 iptables loopback workaround is not needed.

The Forza protocol carries `engine_max_rpm` and `engine_idle_rpm` directly in every packet, so unlike BeamNG and AC1 there's no adaptive-redline learning curve — the LED bar is correctly scaled from the first packet.

### Assetto Corsa Competizione

ACC's only network-telemetry surface is the **Broadcasting API** — connection-oriented UDP on port 9000 with a password handshake. It's intended for spectator overlays, so it carries gear, speed, lap, position, weather, but **not** engine RPM. moza-rev includes a parser and example, but driving the LED bar from ACC would still need a Windows-side shared-memory bridge for RPM.

`moza-rev configure` handles enablement: it edits `Documents/Assetto Corsa Competizione/Config/broadcasting.json` (UTF-16 LE) to set `updListenerPort: 9000` and `connectionPassword: "moza"` — yes, that key really is `upd`-not-`udp`, a Kunos typo we have to match exactly or ACC ignores the value. Confirmation + `.bak` as usual. **Close ACC before applying** — a running game rewrites the file on exit.

The example connects, registers, and logs session and focused-car updates. With the default `configure`-written password it needs no flags:

```sh
cargo run --example assetto_corsa_competizione_log
# pass --password <pwd> if you set a different connectionPassword
```

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
--ams2-port   5606       # change AMS2 / PC2 (Madness) UDP port
--beamng-port 4444       # change BeamNG OutGauge UDP port
--ac-port     9996       # change Assetto Corsa UDP port
--forza-port  9999       # change Forza Data Out UDP port
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
cargo run --example wreckfest_2_log      # one-line summary per Wreckfest 2 telemetry packet
cargo run --example dirt_rally_2_log     # one-line summary per Codemasters EGO packet
cargo run --example beamng_log           # one-line summary per OutGauge packet
cargo run --example automobilista_2_log  # one-line summary per AMS2 / PC2 telemetry packet
cargo run --example assetto_corsa_log    # handshake + one-line summary per RTCarInfo packet
cargo run --example assetto_corsa_competizione_log
                                         # ACC Broadcasting API session + focused-car updates
cargo run --example forza_log            # Forza Data Out (FM7 / FH4 / FH5)
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
