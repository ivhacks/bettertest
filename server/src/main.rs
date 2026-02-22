mod boss;
mod worker;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    worker: bool,
    #[arg(long)]
    boss: bool,
    #[arg(long)]
    pipedef: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match (args.boss, args.worker) {
        (true, true) => panic!("pick one bro: --boss or --worker, not both"),
        (false, false) => panic!("need either --boss or --worker"),
        (true, false) => {
            let pipedef = args
                .pipedef
                .expect("--boss requires --pipedef <path>");
            boss::run(&pipedef).await
        }
        (false, true) => worker::run().await,
    }
}
