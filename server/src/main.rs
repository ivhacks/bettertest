mod boss;
mod worker;

use aide::openapi::{Info, OpenApi};
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
    #[arg(long)]
    dump_api: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if args.dump_api {
        dump_api_specs();
        return;
    }

    match (args.boss, args.worker) {
        (true, true) => panic!("pick one bro: --boss or --worker, not both"),
        (false, false) => panic!("need either --boss or --worker"),
        (true, false) => {
            let pipedef = args.pipedef.expect("--boss requires --pipedef <path>");
            boss::run(&pipedef).await
        }
        (false, true) => worker::run().await,
    }
}

fn dump_api_specs() {
    // worker spec
    let mut worker_api = OpenApi {
        info: Info {
            title: "bettertest worker API".into(),
            version: "0.1.0".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = worker::api_routes().finish_api(&mut worker_api);
    let worker_yaml = serde_yaml::to_string(&worker_api).expect("failed to serialize worker api");
    std::fs::write("worker-api.yaml", &worker_yaml).expect("failed to write worker-api.yaml");
    println!("wrote worker-api.yaml");

    // boss spec
    let mut boss_api = OpenApi {
        info: Info {
            title: "bettertest boss API".into(),
            version: "0.1.0".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = boss::api_routes().finish_api(&mut boss_api);
    let boss_yaml = serde_yaml::to_string(&boss_api).expect("failed to serialize boss api");
    std::fs::write("boss-api.yaml", &boss_yaml).expect("failed to write boss-api.yaml");
    println!("wrote boss-api.yaml");
}
