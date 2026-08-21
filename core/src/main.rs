mod boss;
mod dispatch;
mod worker;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Boss,
    Worker,
    Dispatch,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Args::parse().command {
        Command::Boss => boss::entry().await?,
        Command::Worker => worker::entry(),
        Command::Dispatch => dispatch::entry(),
    }
    Ok(())
}
