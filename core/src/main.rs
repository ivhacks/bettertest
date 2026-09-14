mod boss;
mod db;
mod dispatch;
mod dtos;
mod embedded_scripts;
mod pipedef;
mod worker;

use clap::*;
use std::{error::Error, path::PathBuf};

#[derive(Parser)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Boss { pipedef: PathBuf },
    Worker,
    Dispatch { pipedef: PathBuf },
    Unified { pipedef: PathBuf },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match Args::parse().command {
        Command::Boss { pipedef } => boss::entry(pipedef).await?,
        Command::Worker => worker::entry().await?,
        Command::Dispatch { pipedef } => dispatch::entry(pipedef),
        Command::Unified { pipedef } => {
            tokio::try_join!(worker::entry(), boss::entry(pipedef))?;
        }
    }
    Ok(())
}
