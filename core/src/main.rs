mod boss;
mod dispatch;
mod pipedef;
mod worker;

use clap::*;
use std::path::PathBuf;

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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Args::parse().command {
        Command::Boss { pipedef } => boss::entry(pipedef).await?,
        Command::Worker => worker::entry(),
        Command::Dispatch { pipedef } => dispatch::entry(pipedef),
    }
    Ok(())
}
