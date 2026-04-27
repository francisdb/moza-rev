# moza-rev
Control the moza race wheel rev meter with game telemetry

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

Default UDP port is `23123` — matches `moza-rev`'s default. Override with `--port` on either side if needed.

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
