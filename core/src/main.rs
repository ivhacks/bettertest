mod auth;
mod boss;
mod client;
mod db;
mod dispatch;
mod passkey;
mod pipedef;
mod repo;
mod tls;
mod vm;
mod workdir;
mod worker;

use clap::*;
use std::{
    error::Error,
    path::PathBuf,
    process::ExitCode,
    sync::{Mutex, MutexGuard, PoisonError},
};

#[derive(Parser)]
#[command(version, about, long_about = None, arg_required_else_help = true)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the web ui and the API on port 7357, and run pipelines.
    ///
    /// Each pipedef is one pipeline, named after its file. A directory stands for every *.py file
    /// in it, and a git repo url (credentials included, for its forge) for every .bettertest/*.py
    /// file of its default branch.
    Boss {
        /// Pipedef files, directories (every *.py in them), and git repo urls
        /// (http://user:token@host/owner/name.git: every .bettertest/*.py of the default branch).
        #[arg(required_unless_present = "sources_file")]
        sources: Vec<String>,

        /// A file of sources, one per line (# comments allowed): keep repo urls with credentials in
        /// it, readable only by you, so they never show in the process list.
        #[arg(long = "sources")]
        sources_file: Option<PathBuf>,
        #[command(flatten)]
        https: Https,
    },
    /// Run tasks for the boss on port 7358.
    Worker {
        /// A label tasks can pick this worker by, besides its architecture's (x86 or arm), like
        /// gpu or pcba-rig. Give it once per label.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// How many tasks it runs at once. One unless the machine is plain compute: a fixture with
        /// hardware on it can rarely run two tests at the same time.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        tasks: u32,
    },
    /// Run one task on its worker, without a boss, and print its log.
    ///
    /// Exits 0 if the task passed and 1 if it failed.
    Dispatch {
        /// The pipedef file.
        pipedef: PathBuf,

        /// The stage the task is in (its class name).
        #[arg(short, long)]
        stage: String,

        /// The task (its method name).
        #[arg(short, long)]
        task: String,

        /// The worker's url, for a task that picks its worker by labels. Overrides the one the
        /// pipedef names.
        #[arg(short, long)]
        worker: Option<String>,
    },
    /// Print a pipedef's stages and tasks, as JSON. Touches no worker.
    Parse {
        /// The pipedef file.
        pipedef: PathBuf,
    },
    /// Boss and worker in one process: bettertest on one machine.
    Unified {
        /// Pipedef files, directories (every *.py in them), and git repo urls
        /// (http://user:token@host/owner/name.git: every .bettertest/*.py of the default branch).
        #[arg(required = true)]
        sources: Vec<String>,

        #[command(flatten)]
        https: Https,
    },
}

/// HTTPS, off unless both are given. bettertest makes no certificates: bring one the browsers and
/// workers that use the boss already trust.
#[derive(clap::Args)]
struct Https {
    /// The boss's certificate, PEM, intermediates after it. Turns on HTTPS.
    #[arg(long, requires = "key")]
    cert: Option<PathBuf>,

    /// The certificate's private key, PEM.
    #[arg(long, requires = "cert")]
    key: Option<PathBuf>,
}

impl Https {
    fn load(&self) -> Result<Option<tls::Tls>, Box<dyn Error>> {
        match (&self.cert, &self.key) {
            (Some(cert), Some(key)) => Ok(Some(tls::load(cert, key)?)),
            _ => Ok(None),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let ran: Result<ExitCode, Box<dyn Error>> = match Args::parse().command {
        Command::Boss {
            mut sources,
            sources_file,
            https,
        } => match (
            sources_file.map(|file| read_sources(&file)).transpose(),
            https.load(),
        ) {
            (Err(e), _) | (_, Err(e)) => Err(e),
            (Ok(more), Ok(tls)) => {
                sources.extend(more.unwrap_or_default());
                boss::entry(sources, tls).await.map(|()| ExitCode::SUCCESS)
            }
        },
        Command::Worker { labels, tasks } => worker::entry(labels, tasks)
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::Dispatch {
            pipedef,
            stage,
            task,
            worker,
        } => dispatch::entry(&pipedef, &stage, &task, worker).await,
        Command::Parse { pipedef } => dispatch::parse(&pipedef).await,
        Command::Unified { sources, https } => match https.load() {
            Ok(tls) => tokio::try_join!(worker::entry(vec![], 1), boss::entry(sources, tls))
                .map(|_| ExitCode::SUCCESS),
            Err(e) => Err(e),
        },
    };
    match ran {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The sources in `file`: one per line, ignoring blank lines and lines starting with #.
fn read_sources(file: &std::path::Path) -> Result<Vec<String>, Box<dyn Error>> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| format!("could not read sources from {}: {e}", file.display()))?;
    let sources: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect();
    if sources.is_empty() {
        return Err(format!("{} lists no sources", file.display()).into());
    }
    Ok(sources)
}

/// Locks a mutex. A poisoned mutex only means another thread panicked while holding it, and with
/// panic = "abort" there is no other thread left to have done that, so the data is fine as is.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
