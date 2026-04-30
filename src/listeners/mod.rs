// Per-game telemetry listeners. Each submodule owns the binding,
// receive loop, and protocol-to-runtime conversion for one game family.
//
// Boundary with the protocol modules (`crate::wreckfest`, `crate::madness`,
// etc.): protocol modules are wire-format only - they parse bytes into
// their own native structs and know nothing about channels, threads, or
// `EngineState`. Listener modules glue the parser to the runtime by
// wrapping the listening socket, converting the parsed packet into the
// shared `EngineState`, and forwarding it through an `Update` channel.
//
// This split keeps protocol modules cleanly extractable into a separate
// crate later without dragging the runtime concerns along.
//
// Each listener's `info!` / `warn!` / `error!` calls inherit their
// module path as the log target, so journal output looks like:
//
//     [INFO  moza_rev::listeners::assetto_corsa] connected: car="..." track="..."
//     [INFO  moza_rev::app] telemetry stream from Assetto Corsa started
//
// - different log targets disambiguate the source, and the wording
// distinguishes the handshake-layer event from the stream-layer event.

pub mod assetto_corsa;
pub mod codemasters_legacy;
pub mod forza;
pub mod madness;
pub mod outgauge;
pub mod wreckfest_2;

/// Engine RPM band the LED-driving runtime needs. Each listener
/// constructs one of these from its game's wire format.
#[derive(Debug, Clone, Copy)]
pub struct EngineState {
    pub rpm: i32,
    pub rpm_redline: i32,
    pub rpm_idle: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameId {
    Wreckfest2,
    /// Any Codemasters EGO-engine title using the "extradata" UDP format
    /// (DiRT 2/3/Showdown, F1 2010-2017, GRID series, DiRT Rally, DR2).
    CodemastersLegacy,
    /// BeamNG.drive (also Live For Speed and other LFS-OutGauge-compat
    /// titles, as long as they target the same UDP port).
    BeamNG,
    /// Automobilista 2 / Project CARS 2 (and PC1, PC3) - Madness-engine
    /// PC2 UDP format.
    Ams2,
    /// Assetto Corsa 1 - handshake-based UDP, adaptive redline.
    AssettoCorsa,
    /// Forza Data Out (FM7 / FH4 / FH5). RPM, redline and idle are all
    /// in-protocol so no adaptive tracking is needed.
    Forza,
}

impl GameId {
    /// Compact tag for the per-tick status line (`[WF2]`, `[AC]`, …).
    pub fn label(self) -> &'static str {
        match self {
            GameId::Wreckfest2 => "WF2",
            GameId::CodemastersLegacy => "DR2",
            GameId::BeamNG => "BNG",
            GameId::Ams2 => "AMS2",
            GameId::AssettoCorsa => "AC",
            GameId::Forza => "FZA",
        }
    }

    /// Full name for startup / lifecycle log lines.
    pub fn name(self) -> &'static str {
        match self {
            GameId::Wreckfest2 => "Wreckfest 2",
            GameId::CodemastersLegacy => "Codemasters EGO (DR2 / DiRT / F1 / GRID)",
            GameId::BeamNG => "BeamNG.drive",
            GameId::Ams2 => "Automobilista 2 / Project CARS 2",
            GameId::AssettoCorsa => "Assetto Corsa",
            GameId::Forza => "Forza (FM7 / FH4 / FH5)",
        }
    }
}

/// One per-tick telemetry update flowing from a listener thread to the
/// runtime, identifying which game produced it and the engine state to
/// drive the LED bar with.
#[derive(Debug, Clone, Copy)]
pub struct Update {
    pub game: GameId,
    pub engine: EngineState,
}
