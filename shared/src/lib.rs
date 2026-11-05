use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

// Pipeline structure, as dumped by the pipedef. This is the definition, not an instance.

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    // Every label a worker must have. The boss picks one whose labels cover this set.
    pub labels: Vec<String>,
    pub image: Option<String>,
    pub vm: Option<String>,
    // The vm's architecture. None means the worker's own.
    pub arch: Option<Arch>,
    // How many seconds the task body may take. None means as long as it likes.
    pub timeout: Option<f64>,
    // The name of the pipeline's sandbox it runs in, one tasks of other stages share too.
    pub sandbox: Option<String>,
}

/// The machine a vm emulates. A worker runs its own at full speed (kvm), any other slowly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The label every worker of this architecture has.
    pub fn label(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86",
            Arch::Aarch64 => "arm",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    // When set, every task of the stage runs in this one sandbox, one task at a time, in order.
    // Otherwise each task runs on its own, all at the same time.
    pub sandbox: Option<Sandbox>,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sandbox {
    // As in Task: every label a worker must have.
    pub labels: Vec<String>,
    pub image: Option<String>,
    pub vm: Option<String>,
    pub arch: Option<Arch>,
    // Its tasks run at the same time, in the one sandbox, rather than one at a time, in order.
    pub parallel: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
    // Sandboxes that tasks of several stages share, by name. Each lives from the first stage that
    // uses it to the last.
    pub sandboxes: BTreeMap<String, Sandbox>,
}

// One execution of a pipeline. A run captures its own stage/task structure when it is created,
// so history stays intact when the pipedef changes later.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Pass,
    Fail,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Running,
    Pass,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: Uuid,
    pub name: String,
    pub state: TaskState,
    // The worker it ran on. None until it starts.
    pub worker: Option<String>,
    // When it started and ended, in milliseconds since 1970 by the boss's clock.
    pub started: Option<u64>,
    pub ended: Option<u64>,
    // In a rerun, a task it did not run again: the task run whose result it carries over.
    pub carried_from: Option<Uuid>,
}

impl TaskRun {
    /// How long it ran, or has been running by `now`. None if it never started.
    pub fn runtime(&self, now: u64) -> Option<u64> {
        let ended = match self.state {
            TaskState::Running => now,
            _ => self.ended?,
        };
        Some(ended.saturating_sub(self.started?))
    }
}

impl TaskState {
    pub fn is_over(self) -> bool {
        matches!(self, Self::Pass | Self::Fail | Self::Skipped)
    }
}

/// "3.4s", "42s", "1m 23s", "1h 02m": milliseconds, for people to read.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..10 => format!("{}.{}s", seconds, ms % 1000 / 100),
        10..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageRun {
    pub name: String,
    pub tasks: Vec<TaskRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub pipeline: String,
    pub state: RunState,
    pub stages: Vec<StageRun>,
    // What the run tested, when its pipeline lives in a git repo.
    pub commit: Option<Commit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub branch: String,
}

impl Commit {
    /// "main @ 3fe66e3", for people to read.
    pub fn label(&self) -> String {
        let short: String = self.sha.chars().take(7).collect();
        format!("{} @ {short}", self.branch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
    // Lines bettertest itself writes into a task's log, e.g. "starting container debian:bookworm".
    Bettertest,
}

/// A file a task published with artifact(). The boss keeps it once the task is done, because the
/// worker that made it may be gone by the time anyone wants it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub size: u64,
}

/// Artifact names end up in file paths and URLs.
pub fn valid_artifact_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub stream: LogStream,
    pub line: String,
}

// The same lowercase names go over the wire, into the db, and into CSS classes.
macro_rules! names {
    ($ty:ident { $($variant:ident = $name:literal),+ $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($name => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

names!(Arch { X86_64 = "x86_64", Aarch64 = "aarch64" });
names!(Role { None = "none", Read = "read", Write = "write", Admin = "admin" });
names!(TaskState { Pending = "pending", Running = "running", Pass = "pass", Fail = "fail", Skipped = "skipped" });
names!(RunState { Running = "running", Pass = "pass", Fail = "fail" });
names!(LogStream { Stdout = "stdout", Stderr = "stderr", Bettertest = "bettertest" });

// How many runs the pipeline page shows.
pub const RECENT_RUNS: usize = 50;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineSnapshot {
    pub name: String,
    // Every pipeline the boss knows, for the tabs.
    pub pipelines: Vec<String>,
    // The pipedef as it is right now. Err holds python's complaint when it does not parse.
    pub pipeline: Result<Pipeline, String>,
    // Newest first.
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    // The boss's clock when it took the snapshot, to tell how long a running task has been at it.
    pub now: u64,
    pub run: Run,
    // For a task a rerun carried over: the run that really ran it. The log and artifacts are that
    // run's.
    pub carried_from: Option<Uuid>,
    // When retention deleted the task's log and its artifacts, if it has.
    pub log_deleted: Option<u64>,
    pub artifacts_deleted: Option<u64>,
    pub stage: String,
    pub task: TaskRun,
    pub lines: Vec<LogLine>,
    pub artifacts: Vec<Artifact>,
}

/// What people can change on the settings page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    // How many days after a task ends its artifacts and its log are kept. None keeps them forever.
    pub artifact_days: Option<u32>,
    pub log_days: Option<u32>,
}

/// What a user may do, each role everything the one before it may. The logged-out user has one too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    // Nothing at all: not even look.
    None,
    Read,
    // Start and rerun runs, add workers, upload artifacts.
    Write,
    // Settings and users too.
    Admin,
}

/// Who the browser is: its user, or the logged-out user when it has not logged in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub name: String,
    // What it may do: its user's role, or the logged-out user's if that is more.
    pub role: Role,
    pub logged_in: bool,
}

/// A row of the users page: a user and how many ways of logging in of each kind it has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub role: Role,
    // The logged-out user: always there, its role the only thing about it to change.
    pub builtin: bool,
    pub passwords: u32,
    pub certificates: u32,
    pub passkeys: u32,
    // API tokens, for scripts and curl.
    pub tokens: u32,
    // Its client certificates, for certificate login.
    pub certs: Vec<Certificate>,
    // Its passkeys.
    pub keys: Vec<Passkey>,
}

/// A client certificate a user logs in with: the credential's id, its name, and when it expires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Certificate {
    pub id: i64,
    pub label: String,
    pub expires: String,
}

/// The most client certificates one user may have.
pub const MOST_CERTIFICATES: usize = 5;

/// A passkey a user logs in with: the credential's id and its name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Passkey {
    pub id: i64,
    pub label: String,
}

/// The most passkeys one user may have.
pub const MOST_PASSKEYS: usize = 5;

/// The certificate the boss presents over HTTPS, for the settings page. None: plain HTTP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Https {
    // The names it covers.
    pub names: Vec<String>,
    pub expires: String,
    pub days_left: i32,
}

/// A worker the boss knows: one a pipedef named, or one added on the workers page. The boss keeps
/// a connection to each (the worker's `/events` stream) for as long as it can, which is what makes
/// it connected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Worker {
    pub url: String,
    // What the worker said about itself. None while the boss cannot reach it.
    pub info: Option<WorkerInfo>,
    // When it last connected or disconnected, in milliseconds since 1970 by the boss's clock.
    pub since: u64,
    // Why the boss cannot reach it, once it has tried.
    pub problem: Option<String>,
    // The tasks running on it now.
    pub tasks: Vec<WorkerTask>,
}

/// What a worker says about itself when the boss connects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub hostname: String,
    pub arch: Arch,
    pub version: String,
    // What tasks pick it by: its architecture's (x86 or arm), and any it was started with.
    pub labels: Vec<String>,
    // How many tasks it runs at once (`worker --tasks`).
    pub capacity: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkerTask {
    pub id: Uuid,
    pub pipeline: String,
    pub run: Uuid,
    pub stage: String,
    pub task: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkersSnapshot {
    // The boss's clock when it took the snapshot, to tell how long a worker has been up or down.
    pub now: u64,
    // Sorted by url.
    pub workers: Vec<Worker>,
}

/// What a live SSE stream carries: first a snapshot of `S`, then events. The boss takes the
/// snapshot and subscribes to events under the same lock writers hold while they write and
/// broadcast, so an event is either already in the snapshot or arrives afterwards. Never both,
/// never neither. Both travel as one kind of SSE message so the browser can never reorder them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Live<S> {
    Snapshot(S),
    Event(Event),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    RunStarted(Run),
    TaskStarted {
        pipeline: String,
        run: Uuid,
        task: Uuid,
        worker: String,
        at: u64,
    },
    TaskState {
        pipeline: String,
        run: Uuid,
        task: Uuid,
        state: TaskState,
        at: u64,
    },
    RunFinished {
        pipeline: String,
        run: Uuid,
        state: RunState,
    },
    Log {
        task: Uuid,
        line: LogLine,
    },
    Artifact {
        task: Uuid,
        artifact: Artifact,
    },
    // Everything about the worker, as it is now.
    Worker(Worker),
    WorkerForgotten {
        url: String,
    },
}

impl Run {
    /// The first 6 hex digits of the id, the way git abbreviates commit hashes. For people to read;
    /// links and the API use the full id.
    pub fn short_id(&self) -> String {
        self.id.simple().to_string().chars().take(6).collect()
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::TaskStarted { run, task, .. } | Event::TaskState { run, task, .. }
                if *run == self.id =>
            {
                if let Some(t) = self.task_mut(*task) {
                    t.apply(event);
                }
            }
            Event::RunFinished { run, state, .. } if *run == self.id => self.state = *state,
            _ => {}
        }
    }

    /// Whether every task of every stage before `stage` passed. Only then did `stage` run, and only
    /// then can a rerun start from it.
    pub fn passed_before(&self, stage: &str) -> bool {
        self.stages
            .iter()
            .take_while(|s| s.name != stage)
            .flat_map(|s| &s.tasks)
            .all(|t| t.state == TaskState::Pass)
    }

    /// The names of task `id` and of its stage.
    pub fn locate(&self, id: Uuid) -> Option<(&str, &str)> {
        self.stages.iter().find_map(|stage| {
            let task = stage.tasks.iter().find(|task| task.id == id)?;
            Some((stage.name.as_str(), task.name.as_str()))
        })
    }

    pub fn task_mut(&mut self, id: Uuid) -> Option<&mut TaskRun> {
        self.stages
            .iter_mut()
            .flat_map(|stage| &mut stage.tasks)
            .find(|task| task.id == id)
    }
}

impl PipelineSnapshot {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::RunStarted(run) if run.pipeline == self.name => {
                self.runs.insert(0, run.clone());
                self.runs.truncate(RECENT_RUNS);
            }
            _ => {
                for run in &mut self.runs {
                    run.apply(event);
                }
            }
        }
    }
}

impl TaskRun {
    /// `event` is about this task.
    fn apply(&mut self, event: &Event) {
        match event {
            Event::TaskStarted { worker, at, .. } => {
                self.state = TaskState::Running;
                self.worker = Some(worker.clone());
                self.started = Some(*at);
            }
            Event::TaskState { state, at, .. } => {
                self.state = *state;
                if state.is_over() {
                    self.ended = Some(*at);
                }
            }
            _ => {}
        }
    }
}

impl TaskSnapshot {
    pub fn apply(&mut self, event: &Event) {
        self.run.apply(event);
        match event {
            Event::TaskStarted { task, .. } | Event::TaskState { task, .. }
                if *task == self.task.id =>
            {
                self.task.apply(event)
            }
            Event::Log { task, line } if *task == self.task.id => self.lines.push(line.clone()),
            Event::Artifact { task, artifact } if *task == self.task.id => {
                self.artifacts.push(artifact.clone())
            }
            _ => {}
        }
    }
}

impl WorkersSnapshot {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Worker(worker) => {
                match self.workers.binary_search_by(|w| w.url.cmp(&worker.url)) {
                    Ok(found) => {
                        if let Some(w) = self.workers.get_mut(found) {
                            *w = worker.clone();
                        }
                    }
                    Err(at) => self.workers.insert(at, worker.clone()),
                }
            }
            Event::WorkerForgotten { url } => self.workers.retain(|w| w.url != *url),
            _ => {}
        }
    }
}
