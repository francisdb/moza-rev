// Binary entrypoint. All the heavy lifting lives in `app::run` (the
// listener orchestration / main loop) and `cli` (the clap surface).

mod app;
mod cli;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use moza_rev::configure;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    if let Some(Command::Configure) = cli.command {
        return configure::run();
    }
    app::run(cli.listen)
}
