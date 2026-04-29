// Command-line surface — all clap-related types live here. The runtime
// (`crate::app`) takes a `ListenArgs` and never sees clap. The binary
// entrypoint (`crate::main`) only does `Cli::parse()` + dispatch.

use clap::{Parser, Subcommand, ValueEnum};

use moza_rev::assetto_corsa;
use moza_rev::codemasters_legacy;
use moza_rev::madness;
use moza_rev::moza::Protocol;
use moza_rev::outgauge;
use moza_rev::wreckfest;

pub const DEFAULT_WF2_PORT: u16 = wreckfest::DEFAULT_PORT; // 23123
pub const DEFAULT_DR2_PORT: u16 = codemasters_legacy::DEFAULT_PORT; // 20777
pub const DEFAULT_BEAMNG_PORT: u16 = outgauge::DEFAULT_PORT; // 4444
pub const DEFAULT_AMS2_PORT: u16 = madness::DEFAULT_PORT; // 5606
pub const DEFAULT_AC_PORT: u16 = assetto_corsa::DEFAULT_PORT; // 9996
pub const DEFAULT_LED_COUNT: usize = 10;

/// drive the Moza wheel's RPM LED bar from game telemetry
///
/// All listeners run simultaneously. The active game is whichever was
/// last to send a packet; the wheel goes idle after ~2s of silence.
/// Use `moza-rev configure` to detect installed games and enable their
/// telemetry output.
#[derive(Parser)]
#[command(name = "moza-rev", version, about, long_about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub listen: ListenArgs,
}

#[derive(Subcommand)]
pub enum Command {
    /// Detect installed games and offer to enable their telemetry.
    Configure,
}

#[derive(clap::Args)]
pub struct ListenArgs {
    /// Wreckfest 2 ("Pino") UDP port.
    #[arg(long, default_value_t = DEFAULT_WF2_PORT, value_name = "PORT")]
    pub wf2_port: u16,

    /// Codemasters EGO UDP port (DR2, DR1, F1 2010-2017, DiRT 2/3/Showdown, GRID).
    #[arg(long, default_value_t = DEFAULT_DR2_PORT, value_name = "PORT")]
    pub dr2_port: u16,

    /// Automobilista 2 / Project CARS 2 (Madness engine) UDP port.
    /// On Linux the limited-broadcast loopback needs an iptables NAT — see README.
    #[arg(long, default_value_t = DEFAULT_AMS2_PORT, value_name = "PORT")]
    pub ams2_port: u16,

    /// BeamNG.drive (OutGauge) / Live For Speed UDP port.
    #[arg(long, default_value_t = DEFAULT_BEAMNG_PORT, value_name = "PORT")]
    pub beamng_port: u16,

    /// Assetto Corsa 1 UDP port (handshake-based; adaptive redline).
    #[arg(long, default_value_t = DEFAULT_AC_PORT, value_name = "PORT")]
    pub ac_port: u16,

    /// Override the autodetected Moza wheelbase serial path.
    #[arg(short, long, value_name = "PATH")]
    pub serial: Option<String>,

    /// Number of LEDs on the wheel.
    #[arg(short, long, default_value_t = DEFAULT_LED_COUNT, value_name = "N")]
    pub leds: usize,

    /// Force a specific Moza wire protocol (default: autodetected from USB id).
    #[arg(long, value_enum, value_name = "PROTOCOL")]
    pub protocol: Option<ProtocolArg>,

    /// Print one status line per packet instead of overwriting in place.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(ValueEnum, Clone, Copy)]
pub enum ProtocolArg {
    /// Modern protocol (R9, R12, …).
    #[value(alias = "new")]
    Modern,
    /// Legacy protocol (R3, R5, ES).
    #[value(aliases = ["old", "es"])]
    Legacy,
}

impl From<ProtocolArg> for Protocol {
    fn from(p: ProtocolArg) -> Self {
        match p {
            ProtocolArg::Modern => Protocol::Modern,
            ProtocolArg::Legacy => Protocol::Legacy,
        }
    }
}
