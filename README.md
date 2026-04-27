# moza-rev
Control the moza race wheel rev meter with game telemetry

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
