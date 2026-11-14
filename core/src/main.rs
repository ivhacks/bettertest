// std's println! and eprintln! panic when the write fails (a full disk under `> boss.log`, a closed
// pipe), and a panic ends the process. These drop the line instead, in every module below.
macro_rules! println {
    ($($t:tt)*) => {{ use std::io::Write as _; let _ = writeln!(std::io::stdout(), $($t)*); }};
}
macro_rules! eprintln {
    ($($t:tt)*) => {{ use std::io::Write as _; let _ = writeln!(std::io::stderr(), $($t)*); }};
}

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
    /// Serve the web ui and the API, and run pipelines.
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
        /// Port the web UI and API listen on.
        #[arg(long, default_value_t = boss::LISTEN_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        boss_port: u16,
        #[command(flatten)]
        workdir: Workdir,
        #[command(flatten)]
        https: Https,
    },
    /// Run tasks for the boss.
    Worker {
        /// A label tasks can pick this worker by, besides its architecture's (x86 or arm), like
        /// gpu or pcba-rig. Give it once per label.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// How many tasks it runs at once. One unless the machine is plain compute: a fixture with
        /// hardware on it can rarely run two tests at the same time.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        tasks: u32,
        /// Port this worker listens on.
        #[arg(long, default_value_t = worker::LISTEN_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        worker_port: u16,
        #[command(flatten)]
        workdir: Workdir,
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
    ///
    /// Enrolls that worker at http://[::1]:<worker port>, so a pipedef that only names labels has
    /// this machine to run on.
    Unified {
        /// Pipedef files, directories (every *.py in them), and git repo urls
        /// (http://user:token@host/owner/name.git: every .bettertest/*.py of the default branch).
        #[arg(required = true)]
        sources: Vec<String>,

        /// Port the boss's web UI and API listen on.
        #[arg(long, default_value_t = boss::LISTEN_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        boss_port: u16,
        /// Port the worker listens on.
        #[arg(long, default_value_t = worker::LISTEN_PORT, value_parser = clap::value_parser!(u16).range(1..))]
        worker_port: u16,
        #[command(flatten)]
        workdir: Workdir,
        #[command(flatten)]
        https: Https,
    },
}

/// Where the db and the files around it live. `/var/tmp/bettertest` when omitted.
#[derive(clap::Args)]
struct Workdir {
    /// Directory for the db, artifacts and task files. `/var/tmp/bettertest` when omitted: it
    /// survives a reboot, and every user on the machine can write it.
    #[arg(long)]
    workdir: Option<PathBuf>,
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
    match run(Args::parse().command).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<ExitCode, Box<dyn Error>> {
    if let Some(path) = command.workdir() {
        workdir::choose(path)?;
    }
    match command {
        Command::Boss {
            mut sources,
            sources_file,
            boss_port,
            https,
            ..
        } => {
            if let Some(file) = sources_file {
                sources.extend(read_sources(&file)?);
            }
            boss::entry(sources, https.load()?, boss_port, None).await?;
        }
        Command::Worker {
            labels,
            tasks,
            worker_port,
            ..
        } => worker::entry(labels, tasks, worker_port).await?,
        Command::Dispatch {
            pipedef,
            stage,
            task,
            worker,
        } => return dispatch::entry(&pipedef, &stage, &task, worker).await,
        Command::Parse { pipedef } => return dispatch::parse(&pipedef).await,
        Command::Unified {
            sources,
            boss_port,
            worker_port,
            https,
            ..
        } => {
            let tls = https.load()?;
            // Plain HTTP even when the boss speaks HTTPS. The worker has no certificate; the boss
            // connects here, and the task calls the boss back at the address that connection came
            // from, on --boss-port, over HTTPS when the boss presented one.
            let local = worker::listen_url(worker_port);
            tokio::try_join!(
                worker::entry(vec![], 1, worker_port),
                boss::entry(sources, tls, boss_port, Some(&local))
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

impl Command {
    fn workdir(&self) -> Option<PathBuf> {
        match self {
            Command::Boss { workdir, .. }
            | Command::Worker { workdir, .. }
            | Command::Unified { workdir, .. } => workdir.workdir.clone(),
            _ => None,
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

/// For every accepted connection: send what is written at once. Otherwise a reply written in
/// pieces, say headers then body, waits about 40ms for the client's delayed ack. A socket that
/// refuses is only slower.
pub fn no_delay(stream: &mut tokio::net::TcpStream) {
    let _ = stream.set_nodelay(true);
}

#[cfg(test)]
#[path = "../test/main.rs"]
mod tests;
