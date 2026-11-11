// Runs: starting them (by hand, by push, as reruns), running their stages and tasks, cancelling
// them, and what the pages ask about runs and tasks.

use super::*;

pub(super) struct Going {
    pub(super) pipeline: String,
    pub(super) commit: Option<Commit>,
    // Set to the reason to cancel the run.
    pub(super) cancel: watch::Sender<Option<String>>,
}

/// Says why, once a run is cancelled.
pub(super) type Cancelled = watch::Receiver<Option<String>>;

/// Reads and parses the pipedef fresh every time, so edits show up without restarting the boss. A
/// pipedef in a repo is read at `commit`, or else from the default branch as of the last fetch. A
/// pipedef file has no history: it is read as it is now.
pub(super) async fn load_pipeline(
    boss: &Boss,
    name: &str,
    commit: Option<&Commit>,
) -> Result<(String, Result<Pipeline, String>), Failure> {
    let pipedef = boss
        .pipedef(name)
        .ok_or_else(|| not_found(format!("pipeline {name}")))?;
    let source = match &pipedef {
        Pipedef::File(path) => tokio::fs::read_to_string(path)
            .await
            .map_err(|e| internal(format!("could not read {}: {e}", path.display())))?,
        Pipedef::Repo { repo, path } => {
            let at = commit.map_or("HEAD", |commit| commit.sha.as_str());
            match boss.repo(*repo)?.show(at, path).await {
                Ok(source) => source,
                Err(e) => return Ok((String::new(), Err(e))),
            }
        }
    };
    let pipeline = pipedef::parse(&source).await;
    Ok((source, pipeline))
}

pub(super) async fn get_pipelines(State(boss): State<Shared>) -> Json<Vec<String>> {
    Json(boss.pipelines())
}

pub(super) async fn get_pipeline(
    State(boss): State<Shared>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<Pipeline>, Failure> {
    let (_, pipeline) = load_pipeline(&boss, &name, None).await?;
    pipeline
        .map(Json)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))
}

pub(super) async fn pipeline_events(
    State(boss): State<Shared>,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, Failure> {
    let (_, pipeline) = load_pipeline(&boss, &name, None).await?;
    if let Ok(pipeline) = &pipeline {
        for url in pipeline.workers() {
            boss.enroll(url);
        }
    }
    let pipelines = boss.pipelines();
    let snapshot = move |db: &Db| {
        Ok(Some(PipelineSnapshot {
            name: name.clone(),
            pipelines: pipelines.clone(),
            pipeline: pipeline.clone().map(Arc::new),
            runs: db
                .runs(&name, RECENT_RUNS as i64)?
                .into_iter()
                .map(Arc::new)
                .collect(),
        }))
    };
    let wanted = |snapshot: &PipelineSnapshot, event: &Event| {
        event
            .run()
            .is_some_and(|(pipeline, _)| pipeline == snapshot.name)
    };
    live(boss, snapshot, wanted)
}

/// Starts a run of `name`. A pipeline in a repo runs at the newest commit of the default branch.
pub(super) async fn start_run(
    State(boss): State<Shared>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Run>, Failure> {
    let commit = match boss.pipedef(&name) {
        Some(Pipedef::Repo { repo, .. }) => {
            boss.fetch(repo).await;
            Some(boss.repo(repo)?.head().await.map_err(internal)?)
        }
        _ => None,
    };
    let (source, pipeline) = load_pipeline(&boss, &name, commit.as_ref()).await?;
    let pipeline = pipeline.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let link = link(&boss, &headers, &name);
    let run = begin_run(
        &boss,
        &name,
        pipeline,
        source,
        commit,
        link,
        &BTreeMap::new(),
    )?;
    Ok(Json(run))
}

/// Where people see pipeline `name`, as the one who called us reached us.
pub(super) fn link(boss: &Boss, headers: &HeaderMap, name: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let scheme = if boss.tls.is_some() { "https" } else { "http" };
    Some(format!("{scheme}://{host}/pipeline/{name}"))
}

pub(super) fn begin_run(
    boss: &Shared,
    name: &str,
    pipeline: Pipeline,
    source: String,
    commit: Option<Commit>,
    link: Option<String>,
    carried: &BTreeMap<(String, String), TaskRun>,
) -> Result<Run, Failure> {
    for url in pipeline.workers() {
        boss.enroll(url);
    }
    let run = boss
        .record(|db| {
            let run = db.insert_run(name, &pipeline, commit, carried)?;
            Ok((Event::RunStarted(run.clone()), run))
        })
        .map_err(internal)?;
    let (cancel, cancelled) = watch::channel(None);
    {
        let mut going = lock(&boss.going);
        // Only the newest commit of a branch is worth finishing: cancel runs of older ones.
        if let Some(new) = &run.commit {
            let reason = format!("cancelled: {} replaced it", new.label());
            for other in going.values() {
                let replaced = other.pipeline == run.pipeline
                    && other
                        .commit
                        .as_ref()
                        .is_some_and(|old| old.branch == new.branch && old.sha != new.sha);
                if replaced {
                    other.cancel.send_replace(Some(reason.clone()));
                }
            }
        }
        let this = Going {
            pipeline: run.pipeline.clone(),
            commit: run.commit.clone(),
            cancel,
        };
        going.insert(run.id, this);
    }
    spawn(execute_run(
        boss.clone(),
        run.clone(),
        pipeline,
        source,
        link,
        cancelled,
    ));
    Ok(run)
}

#[derive(Deserialize)]
pub(super) struct Push {
    #[serde(rename = "ref")]
    pub(super) git_ref: String,
    pub(super) after: String,
    pub(super) repository: PushedRepo,
}

#[derive(Deserialize)]
pub(super) struct PushedRepo {
    pub(super) full_name: String,
}

/// A forge's push webhook (Forgejo, Gitea): fetches the repo and starts every pipeline the pushed
/// commit has, at that commit. Returns the runs.
pub(super) async fn push_hook(
    State(boss): State<Shared>,
    headers: HeaderMap,
    Json(push): Json<Push>,
) -> Result<Json<Vec<Run>>, Failure> {
    // A tag, or a deleted branch, has nothing to run.
    let Some(branch) = push.git_ref.strip_prefix("refs/heads/") else {
        return Ok(Json(vec![]));
    };
    if push.after.chars().all(|c| c == '0') {
        return Ok(Json(vec![]));
    }
    let index = boss
        .repos
        .iter()
        .position(|r| r.full_name.eq_ignore_ascii_case(&push.repository.full_name))
        .ok_or_else(|| not_found(format!("repo {}", push.repository.full_name)))?;
    boss.fetch(index).await;
    let repo = boss.repo(index)?;

    let mut runs = vec![];
    let found = repo.pipedefs(&push.after).await.map_err(internal)?;
    for (name, path) in found {
        // Only pipelines this repo owns: another source may have claimed the name.
        if !matches!(boss.pipedef(&name), Some(Pipedef::Repo { repo, .. }) if repo == index) {
            continue;
        }
        let commit = Commit {
            sha: push.after.clone(),
            branch: branch.to_string(),
        };
        let link = link(&boss, &headers, &name);
        let source = repo.show(&commit.sha, &path).await.map_err(internal)?;
        match pipedef::parse(&source).await {
            Ok(pipeline) => runs.push(begin_run(
                &boss,
                &name,
                pipeline,
                source,
                Some(commit),
                link,
                &BTreeMap::new(),
            )?),
            Err(e) => {
                eprintln!("{name} at {}: the pipedef is broken: {e}", commit.label());
                let reported = repo
                    .report(
                        &boss.http,
                        &commit.sha,
                        &name,
                        "failure",
                        "the pipedef is broken",
                        link.as_deref(),
                    )
                    .await;
                if let Err(e) = reported {
                    eprintln!("could not tell {} about it: {e}", repo.public);
                }
            }
        }
    }
    Ok(Json(runs))
}

/// Cancels a running run, all of it: its running tasks are hung up on (which kills them on their
/// workers), sandboxes still starting are stopped once up, and every task still to come is skipped.
/// The same as when a newer commit replaces a run, just asked for by a person.
pub(super) async fn cancel_run(
    State(boss): State<Shared>,
    AxumPath(run): AxumPath<String>,
    ConnectInfo(peer): ConnectInfo<tls::Peer>,
    headers: HeaderMap,
) -> Result<StatusCode, Failure> {
    let id = Uuid::parse_str(&run).map_err(|_| not_found(format!("run {run}")))?;
    let me = whoami(&boss, &headers, Some(&peer))?;
    let reason = match me.logged_in {
        true => format!("cancelled by {}", me.name),
        false => "cancelled from the web ui".to_string(),
    };
    match lock(&boss.going).get(&id) {
        Some(going) => {
            going.cancel.send_replace(Some(reason));
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err((StatusCode::CONFLICT, format!("run {run} is not running"))),
    }
}

/// `/api/rerun/{run}[/{stage}[/{task}]]`: starts a new run of the pipeline at the commit `run`
/// tested, which runs `stage` (or only its `task`) and every stage after it again, and carries over
/// the results of the rest (see `carry_over`). No stage reruns all of it. Returns the new run.
pub(super) async fn rerun(
    State(boss): State<Shared>,
    AxumPath(rerun): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Run>, Failure> {
    let mut parts = rerun.split('/');
    let id = parse_id(parts.next().unwrap_or_default())?;
    let (stage, task) = (parts.next(), parts.next());
    let earlier = boss.db().run(id).map_err(internal)?;
    let earlier = earlier.ok_or_else(|| not_found(format!("run {id}")))?;
    if earlier.state == RunState::Running {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "run {} is still running. Rerun it once it is over.",
                earlier.short_id()
            ),
        ));
    }
    let commit = earlier.commit.as_ref();
    let (source, pipeline) = load_pipeline(&boss, &earlier.pipeline, commit).await?;
    let pipeline = pipeline.map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let carried = carry_over(&earlier, &pipeline, stage, task)?;
    let link = link(&boss, &headers, &earlier.pipeline);
    let name = earlier.pipeline.clone();
    let run = begin_run(
        &boss,
        &name,
        pipeline,
        source,
        earlier.commit,
        link,
        &carried,
    )?;
    Ok(Json(run))
}

/// Where a rerun from stage `at` (or of only its `task`) really starts. A sandbox of the
/// pipeline's lives only as long as its run, so whatever reruns needs every one of its sandboxes
/// built up again from the first stage that uses it: rerunning from there, whole stages.
pub(super) fn rerun_start<'a>(
    pipeline: &Pipeline,
    at: usize,
    task: Option<&'a str>,
) -> (usize, Option<&'a str>) {
    let sandboxes = |stage: &Stage, only: Option<&str>| -> BTreeSet<String> {
        let tasks = stage
            .tasks
            .iter()
            .filter(|t| only.is_none_or(|only| t.name == only));
        tasks.filter_map(|t| t.sandbox.clone()).collect()
    };
    let first_use = |name: &str| {
        let uses = |s: &Stage| s.tasks.iter().any(|t| t.sandbox.as_deref() == Some(name));
        pipeline.stages.iter().position(uses)
    };
    let mut start = at;
    loop {
        // What reruns: from `start` on, but of stage `at` only `task`, while `start` is `at`.
        let mut needed = BTreeSet::new();
        for (index, stage) in pipeline.stages.iter().enumerate().skip(start) {
            let only = if index == at && start == at {
                task
            } else {
                None
            };
            needed.extend(sandboxes(stage, only));
        }
        let earliest = needed
            .iter()
            .filter_map(|n| first_use(n))
            .min()
            .unwrap_or(start);
        if earliest >= start {
            break;
        }
        start = earliest;
    }
    // A task that runs in a sandbox builds on the tasks before it there: they rerun with it.
    let in_sandbox = pipeline
        .stages
        .get(at)
        .is_some_and(|s| !sandboxes(s, task).is_empty());
    match start == at && !(task.is_some() && in_sandbox) {
        true => (at, task),
        false => (start, None),
    }
}

/// The tasks of `earlier` a rerun of `stage` (or of only its `task`) carries over instead of running
/// again, by (stage, task). Every stage before `stage` is carried over, so each of its tasks must
/// have passed: the stages after it ran on that result. So are the other tasks of `stage` when only
/// `task` reruns, unless the stage shares a sandbox, where each task builds on the one before and
/// the whole stage reruns. A task the pipedef has gained since runs, and so does the rest of a
/// shared stage it is in. No `stage` carries nothing over.
pub(super) fn carry_over(
    earlier: &Run,
    pipeline: &Pipeline,
    stage: Option<&str>,
    task: Option<&str>,
) -> Result<BTreeMap<(String, String), TaskRun>, Failure> {
    let mut carried = BTreeMap::new();
    let Some(stage) = stage else {
        return Ok(carried);
    };
    let Some(at) = pipeline.stages.iter().position(|s| s.name == stage) else {
        return Err(not_found(format!("stage {stage} in {}", earlier.pipeline)));
    };
    if let Some(task) = task
        && !pipeline
            .stages
            .get(at)
            .is_some_and(|s| s.tasks.iter().any(|t| t.name == task))
    {
        return Err(not_found(format!(
            "task {stage}.{task} in {}",
            earlier.pipeline
        )));
    }
    let (at, task) = rerun_start(pipeline, at, task);
    let earlier_task = |stage: &str, task: &str| {
        let stage = earlier.stages.iter().find(|s| s.name == stage)?;
        stage.tasks.iter().find(|t| t.name == task)
    };
    for (index, this) in pipeline.stages.iter().enumerate().take(at + 1) {
        let mut carried_here = vec![];
        for t in &this.tasks {
            let Some(was) = earlier_task(&this.name, &t.name) else {
                continue;
            };
            if index == at {
                if task.is_some_and(|task| task != t.name) && was.state.is_over() {
                    carried_here.push(was);
                }
                continue;
            }
            if was.state != TaskState::Pass {
                let what = match was.state {
                    TaskState::Fail => "failed",
                    _ => "did not run",
                };
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "{}.{} {what} in run {}, so the stages after it have nothing to build \
                         on. Rerun {} instead.",
                        this.name,
                        t.name,
                        earlier.short_id(),
                        this.name
                    ),
                ));
            }
            carried_here.push(was);
        }
        // In a stage whose tasks build on each other in one sandbox, rerunning one reruns them all.
        let all = carried_here.len() == this.tasks.len();
        let sequential = this.sandbox.as_ref().is_some_and(|s| !s.parallel);
        if sequential && !all {
            continue;
        }
        for was in carried_here {
            carried.insert((this.name.clone(), was.name.clone()), was.clone());
        }
    }
    Ok(carried)
}

/// Stages run one at a time, in the order the pipedef defines them. Once one fails, the rest are
/// skipped.
pub(super) async fn execute_run(
    boss: Shared,
    run: Run,
    pipeline: Pipeline,
    source: String,
    link: Option<String>,
    cancelled: Cancelled,
) {
    let link = link.as_deref();
    boss.report(&run, "pending", "running", link).await;
    let r = Running {
        boss,
        run: Arc::new(run),
        source: source.into(),
        cancelled,
        spanning: Spanning::default(),
    };
    let mut failed = false;
    // The run was created from this pipeline, so its stages and tasks line up one to one.
    for (index, (stage, stage_run)) in pipeline.stages.iter().zip(&r.run.stages).enumerate() {
        // Tasks a rerun carried over keep the result they have.
        let fresh = || stage_run.tasks.iter().filter(|t| t.carried_from.is_none());
        if failed || r.is_cancelled() {
            for task_run in fresh() {
                r.skip(task_run.id);
            }
            continue;
        }
        let passed = match &stage.sandbox {
            _ if fresh().next().is_none() => {
                stage_run.tasks.iter().all(|t| t.state == TaskState::Pass)
            }
            Some(sandbox) => run_shared_stage(&r, stage, sandbox, stage_run).await,
            None => run_parallel_stage(&r, &pipeline, index, stage, stage_run).await,
        };
        failed = !passed;
    }
    // A failed or cancelled run leaves sandboxes up that later stages would have used.
    let left = std::mem::take(&mut *lock(&r.spanning));
    for (_, up) in left {
        if let Err(e) = client::stop_sandbox(&r.boss.http, &up.worker, &up.target).await {
            r.boss.log(up.last, LogStream::Bettertest, e);
        }
    }
    let (boss, run) = (&r.boss, &*r.run);
    lock(&boss.going).remove(&run.id);
    let reason = r.cancelled.borrow().clone();
    let state = if failed || reason.is_some() {
        RunState::Fail
    } else {
        RunState::Pass
    };
    boss.set_run_state(run, state);
    match reason {
        Some(reason) => boss.report(run, "warning", &reason, link).await,
        None if failed => boss.report(run, "failure", "failed", link).await,
        None => boss.report(run, "success", "passed", link).await,
    }
}

/// What every task of a run needs to run: the boss, the run, the pipedef's source, the run's
/// cancel, and the pipeline's sandboxes that are up. Cloned into each task, cheaply.
#[derive(Clone)]
pub(super) struct Running {
    pub(super) boss: Shared,
    pub(super) run: Arc<Run>,
    pub(super) source: Arc<str>,
    pub(super) cancelled: Cancelled,
    pub(super) spanning: Spanning,
}

impl Running {
    fn is_cancelled(&self) -> bool {
        self.cancelled.borrow().is_some()
    }

    /// Records that task `id` is over: passed or failed. One cut short because its run was cancelled
    /// did not fail: it is skipped, like the tasks that never started.
    fn end(&self, id: Uuid, passed: bool) {
        let state = match passed {
            true => TaskState::Pass,
            false if self.is_cancelled() => TaskState::Skipped,
            false => TaskState::Fail,
        };
        self.boss.set_task_state(&self.run, id, state);
    }

    fn skip(&self, id: Uuid) {
        self.boss.set_task_state(&self.run, id, TaskState::Skipped);
    }

    /// Ends task `id`, which could not run, saying why in its log. Returns that it did not pass.
    fn refuse(&self, id: Uuid, why: String) -> bool {
        self.boss.log(id, LogStream::Bettertest, why);
        self.end(id, false);
        false
    }

    /// A worker for task `id` (see Boss::claim_worker). `named` is a worker url from the pipedef, or
    /// the worker a sandbox of this run is already on.
    async fn claim(
        &self,
        id: Uuid,
        named: Option<&str>,
        labels: &[String],
    ) -> Result<String, String> {
        let claimed = self
            .boss
            .claim_worker(&self.run, id, named, labels, &self.cancelled);
        claimed.await
    }

    /// Starts `sandbox` on `worker` for task `id`, whose log shows it.
    async fn start_sandbox(
        &self,
        id: Uuid,
        worker: &str,
        sandbox: &Sandbox,
    ) -> Result<client::Target, String> {
        let mut log = |stream, line| self.boss.log(id, stream, line);
        let cancelled = Some(self.cancelled.clone());
        client::start_sandbox(&self.boss.http, worker, sandbox, &mut log, cancelled).await
    }

    /// Runs task `id`'s body on `worker` in `target`, and records how it went.
    async fn run_task(
        &self,
        id: Uuid,
        worker: &str,
        stage: &str,
        task: &Task,
        target: &client::Target,
    ) -> bool {
        let mut io = BossTask::new(&self.boss, &self.run, id, self.cancelled.clone());
        let ran = client::run_task(
            &self.boss.http,
            worker,
            &self.source,
            stage,
            task,
            target,
            &mut io,
        );
        ran.await
    }
}

/// Every task gets a sandbox of its own, and they all run at the same time, but for the tasks of
/// each of the pipeline's sandboxes, which take turns in it (see run_in_sandbox). Those a rerun
/// carried over do not run again, but count.
pub(super) async fn run_parallel_stage(
    r: &Running,
    pipeline: &Pipeline,
    index: usize,
    stage: &Stage,
    stage_run: &StageRun,
) -> bool {
    let mut tasks = JoinSet::new();
    // The tasks of each of the pipeline's sandboxes, in the order the stage defines them.
    let mut groups: Vec<(String, Vec<(Task, Uuid)>)> = vec![];
    for (task, task_run) in stage.tasks.iter().zip(&stage_run.tasks) {
        if task_run.carried_from.is_some() {
            continue;
        }
        if let Some(name) = &task.sandbox {
            match groups.iter_mut().find(|(n, _)| n == name) {
                Some((_, group)) => group.push((task.clone(), task_run.id)),
                None => groups.push((name.clone(), vec![(task.clone(), task_run.id)])),
            }
            continue;
        }
        let (r, stage, task, id) = (r.clone(), stage.clone(), task.clone(), task_run.id);
        tasks.spawn(async move {
            let worker = match r.claim(id, task.worker.as_deref(), &task.labels).await {
                Ok(worker) => worker,
                Err(e) => return r.refuse(id, e),
            };
            let mut io = BossTask::new(&r.boss, &r.run, id, r.cancelled.clone());
            let http = &r.boss.http;
            let passed =
                client::execute_task(http, &worker, &r.source, &stage, &task, &mut io).await;
            drop(io);
            r.end(id, passed);
            passed
        });
    }
    let ends_here = last_used_in(pipeline, index);
    for (name, group) in groups {
        let Some(sandbox) = pipeline.sandboxes.get(&name).cloned() else {
            for (_, id) in group {
                r.refuse(id, format!("the pipeline has no sandbox {name}"));
            }
            return false;
        };
        let last = ends_here.contains(&name);
        let (r, stage) = (r.clone(), stage.name.clone());
        tasks.spawn(async move { run_in_sandbox(&r, &stage, &name, &sandbox, group, last).await });
    }
    let carried = stage_run.tasks.iter().filter(|t| t.carried_from.is_some());
    let mut passed = carried.into_iter().all(|t| t.state == TaskState::Pass);
    while let Some(joined) = tasks.join_next().await {
        if !matches!(joined, Ok(true)) {
            passed = false;
        }
    }
    passed
}

/// The pipeline's sandboxes that are up, by name: tasks of several stages share them.
pub(super) type Spanning = Arc<Mutex<BTreeMap<String, Up>>>;

/// A sandbox of the pipeline's that is up: where, and the last task that used it, whose log shows
/// it stopping when a failed run has to.
pub(super) struct Up {
    pub(super) worker: String,
    pub(super) target: client::Target,
    pub(super) last: Uuid,
}

/// The pipeline's sandboxes that no stage after stage `index` uses: they stop once it is over.
pub(super) fn last_used_in(pipeline: &Pipeline, index: usize) -> BTreeSet<String> {
    let used = |stage: &Stage| -> BTreeSet<String> {
        stage
            .tasks
            .iter()
            .filter_map(|t| t.sandbox.clone())
            .collect()
    };
    let here = pipeline.stages.get(index).map(used).unwrap_or_default();
    let later: BTreeSet<String> = pipeline
        .stages
        .iter()
        .skip(index + 1)
        .flat_map(used)
        .collect();
    here.difference(&later).cloned().collect()
}

/// The tasks of one stage that run in sandbox `name` of the pipeline, one at a time, in order:
/// each builds on what the ones before it did, in this stage or an earlier one. Starts the sandbox
/// if no stage has yet (that shows in the log of the task that needs it first), and stops it when
/// this is the `last` stage to use it. Once one fails, the rest are skipped.
pub(super) async fn run_in_sandbox(
    r: &Running,
    stage: &str,
    name: &str,
    sandbox: &Sandbox,
    group: Vec<(Task, Uuid)>,
    last: bool,
) -> bool {
    let mut passed = true;
    for (task, id) in group {
        if !passed || r.is_cancelled() {
            r.skip(id);
            continue;
        }
        let up = lock(&r.spanning).remove(name);
        // A sandbox already up stays on the worker that has it. The first task uses the url the
        // pipedef named, or is placed by labels.
        let (named, labels) = match &up {
            Some(up) => (Some(up.worker.as_str()), &[][..]),
            None => (sandbox.worker.as_deref(), &sandbox.labels[..]),
        };
        let worker = match r.claim(id, named, labels).await {
            Ok(worker) => worker,
            Err(e) => {
                passed = r.refuse(id, e);
                if let Some(up) = up {
                    lock(&r.spanning).insert(name.to_string(), up);
                }
                continue;
            }
        };
        let target = match up {
            Some(up) => up.target,
            None => match r.start_sandbox(id, &worker, sandbox).await {
                Ok(target) => target,
                Err(e) => {
                    passed = r.refuse(id, e);
                    continue;
                }
            },
        };
        passed = r.run_task(id, &worker, stage, &task, &target).await;
        r.end(id, passed);
        let up = Up {
            worker,
            target,
            last: id,
        };
        lock(&r.spanning).insert(name.to_string(), up);
    }
    let done = if last {
        lock(&r.spanning).remove(name)
    } else {
        None
    };
    if let Some(up) = done {
        // A sandbox that will not stop is leaking worker resources. The stage did not pass cleanly.
        if let Err(e) = client::stop_sandbox(&r.boss.http, &up.worker, &up.target).await {
            r.boss.log(up.last, LogStream::Bettertest, e);
            r.boss.set_task_state(&r.run, up.last, TaskState::Fail);
            return false;
        }
    }
    passed
}

/// The tasks share one sandbox. They run one at a time, in order, each building on the one before,
/// and once one fails the rest are skipped; or, in a `parallel` stage, all at once. Starting the
/// sandbox shows in the first task's log, stopping it in the log of the last task to end. Tasks a
/// rerun carried over (only a `parallel` stage has them) do not run again, but count.
pub(super) async fn run_shared_stage(
    r: &Running,
    stage: &Stage,
    sandbox: &Sandbox,
    stage_run: &StageRun,
) -> bool {
    let (carried, fresh): (Vec<_>, Vec<_>) = (stage.tasks.iter())
        .zip(&stage_run.tasks)
        .partition(|(_, task_run)| task_run.carried_from.is_some());
    let carried_passed = carried.iter().all(|(_, t)| t.state == TaskState::Pass);
    let Some((_, first)) = fresh.first() else {
        return carried_passed;
    };
    // Nothing can run when the sandbox cannot: the first task says why, and the rest are skipped.
    let none_ran = |why: String| {
        r.refuse(first.id, why);
        for (_, task_run) in fresh.iter().skip(1) {
            r.skip(task_run.id);
        }
        false
    };
    let claimed = r.claim(first.id, sandbox.worker.as_deref(), &sandbox.labels);
    let worker = match claimed.await {
        Ok(worker) => worker,
        Err(e) => return none_ran(e),
    };
    let target = match r.start_sandbox(first.id, &worker, sandbox).await {
        Ok(target) => target,
        Err(e) => return none_ran(e),
    };
    if sandbox.parallel {
        return run_together(r, &stage.name, &fresh, &worker, &target).await && carried_passed;
    }

    // The stage holds one slot of its worker from its first task to its sandbox stopping: each task
    // takes it over from the one before, whose end is recorded only then, so no other task gets
    // the worker in between.
    let mut passed = true;
    let mut ended: Option<(Uuid, bool)> = None;
    let mut last = first.id;
    for &(task, task_run) in &fresh {
        if !passed || r.is_cancelled() {
            r.skip(task_run.id);
            continue;
        }
        // The first one is running already: it started the sandbox. The rest run where it does.
        if task_run.id != first.id {
            r.boss.continue_on(&r.run, task_run.id, &worker);
        }
        if let Some((id, passed)) = ended.take() {
            r.end(id, passed);
        }
        passed = r
            .run_task(task_run.id, &worker, &stage.name, task, &target)
            .await;
        ended = Some((task_run.id, passed));
        last = task_run.id;
    }

    // A sandbox that will not stop is leaking worker resources. The stage did not pass cleanly.
    if let Err(e) = client::stop_sandbox(&r.boss.http, &worker, &target).await {
        r.boss.log(last, LogStream::Bettertest, e);
        ended = Some((last, false));
        passed = false;
    }
    if let Some((id, passed)) = ended {
        r.end(id, passed);
    }
    passed
}

/// The tasks of a `parallel` stage, all at once in its sandbox, which is up on `worker` already.
/// Each holds a slot of that worker while it runs (the first has one: it started the sandbox), so a
/// worker that takes fewer tasks at once runs them a few at a time, in the order the pipedef
/// defines them. A failed task fails the stage but stops none of the others.
pub(super) async fn run_together(
    r: &Running,
    stage: &str,
    tasks_here: &[(&Task, &TaskRun)],
    worker: &str,
    target: &client::Target,
) -> bool {
    let first = tasks_here.first().map(|(_, t)| t.id);
    let mut tasks = JoinSet::new();
    let mut passed = true;
    for (task, task_run) in tasks_here {
        let id = task_run.id;
        // One claim at a time, so slots go to the tasks in order, and a freed slot wakes one
        // waiting task rather than all of them.
        if Some(id) != first
            && let Err(e) = r.claim(id, Some(worker), &[]).await
        {
            passed &= r.refuse(id, e);
            continue;
        }
        let (r, stage, task) = (r.clone(), stage.to_string(), (*task).clone());
        let (worker, target) = (worker.to_string(), target.clone());
        tasks.spawn(async move {
            let passed = r.run_task(id, &worker, &stage, &task, &target).await;
            r.end(id, passed);
            (id, passed)
        });
    }
    let mut last = first;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((id, task_passed)) => {
                last = Some(id);
                passed &= task_passed;
            }
            Err(_) => passed = false,
        }
    }
    // A sandbox that will not stop is leaking worker resources. The stage did not pass cleanly.
    if let Err(e) = client::stop_sandbox(&r.boss.http, worker, target).await {
        if let Some(last) = last {
            r.boss.log(last, LogStream::Bettertest, e);
            r.boss.set_task_state(&r.run, last, TaskState::Fail);
        }
        return false;
    }
    passed
}

/// A task of a run, as the client sees it: its log, its artifacts, and the artifacts of the rest
/// of its run.
pub(super) struct BossTask<'a> {
    pub(super) boss: &'a Boss,
    pub(super) run: &'a Run,
    pub(super) id: Uuid,
    pub(super) cancelled: Cancelled,
    // What the task shows the boss when it fetches its checkout and artifacts. Good for as long
    // as this lives, which is as long as the task runs.
    pub(super) token: Option<String>,
}

impl<'a> BossTask<'a> {
    fn new(boss: &'a Boss, run: &'a Run, id: Uuid, cancelled: Cancelled) -> Self {
        // Without a token (no randomness to be had) the task acts as logged out, as it used to.
        let token = auth::new_token().ok().map(|(token, hash)| {
            lock(&boss.task_tokens).insert(hash, id);
            token
        });
        BossTask {
            boss,
            run,
            id,
            cancelled,
            token,
        }
    }
}

impl Drop for BossTask<'_> {
    fn drop(&mut self) {
        if let Some(token) = &self.token {
            lock(&self.boss.task_tokens).remove(&auth::token_hash(token));
        }
    }
}

impl client::TaskIo for BossTask<'_> {
    fn log(&mut self, stream: LogStream, line: String) {
        self.boss.log(self.id, stream, line);
    }

    fn log_lines(&mut self, lines: Vec<LogLine>) {
        self.boss.log_lines(self.id, lines);
    }

    fn tests(&mut self, tests: Tests) {
        let event = Event::TaskTests {
            pipeline: self.run.pipeline.clone(),
            run: self.run.id,
            task: self.id,
            tests,
        };
        let written = self
            .boss
            .record(|db| Ok((event, db.set_tests(self.id, tests)?)));
        if let Err(e) = written {
            eprintln!("could not record the tests of task {}: {e}", self.id);
        }
    }

    fn outputs(&self) -> Result<PathBuf, String> {
        task_artifacts_dir(self.id).map_err(|e| e.to_string())
    }

    fn saved(&mut self, artifact: Artifact) {
        self.boss.add_artifact(self.id, artifact);
    }

    fn run(&self) -> Option<&Run> {
        Some(self.run)
    }

    fn boss_name(&self) -> Option<&str> {
        self.boss.tls.as_ref().map(|tls| tls.name.as_str())
    }

    fn boss_port(&self) -> u16 {
        self.boss.port
    }

    fn cancelled(&self) -> Option<Cancelled> {
        Some(self.cancelled.clone())
    }

    fn boss_token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn wait_when_busy(&self) -> bool {
        true
    }
}

pub(super) async fn get_run(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Run>, Failure> {
    let id = parse_id(&id)?;
    let run = boss.db().run(id).map_err(internal)?;
    run.map(Json).ok_or_else(|| not_found(format!("run {id}")))
}

pub(super) async fn run_events(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let snapshot = move |db: &Db| db.run(id);
    let wanted = |run: &Run, event: &Event| event.run().is_some_and(|(_, id)| id == run.id);
    live(boss, snapshot, wanted)
}

/// The files of the commit run `id` tests, as a tar stream, for its tasks to run in.
pub(super) async fn get_source(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let run = boss.db().run(id).map_err(internal)?;
    let run = run.ok_or_else(|| not_found(format!("run {id}")))?;
    let commit = run
        .commit
        .ok_or_else(|| not_found(format!("commit: run {id} tests none")))?;
    let Some(Pipedef::Repo { repo, .. }) = boss.pipedef(&run.pipeline) else {
        return Err(not_found(format!(
            "repo: pipeline {} is in none",
            run.pipeline
        )));
    };
    let mut archive = boss
        .repo(repo)?
        .archive(&commit.sha)
        .map_err(internal)?
        .spawn()
        .map_err(internal)?;
    let stdout = archive
        .stdout
        .take()
        .ok_or_else(|| internal("git archive has no stdout"))?;
    // The stream reads stdout after this returns, so git must live on until it is done.
    spawn(async move {
        let _ = archive.wait().await;
    });
    let headers = [(header::CONTENT_TYPE, "application/x-tar")];
    Ok((headers, Body::from_stream(ReaderStream::new(stdout))).into_response())
}

/// A task's whole log as plain text, for curl.
pub(super) async fn get_task_log(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<String, Failure> {
    let id = parse_id(&id)?;
    let db = boss.db();
    if db.task(id).map_err(internal)?.is_none() {
        return Err(not_found(format!("task {id}")));
    }
    let lines = db.log(id).map_err(internal)?;
    Ok(lines.into_iter().map(|l| l.line + "\n").collect())
}

pub(super) async fn task_events(
    State(boss): State<Shared>,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, Failure> {
    let id = parse_id(&id)?;
    let snapshot = move |db: &Db| {
        let Some((run, stage)) = db.task(id)? else {
            return Ok(None);
        };
        let Some(task) = run
            .stages
            .iter()
            .flat_map(|s| &s.tasks)
            .find(|t| t.id == id)
            .cloned()
        else {
            return Ok(None);
        };
        // A task a rerun carried over shows the log and artifacts of the run that ran it.
        let ran = task.carried_from.unwrap_or(id);
        let carried_from = match task.carried_from {
            Some(earlier) => db.task(earlier)?.map(|(run, _)| run.id),
            None => None,
        };
        let lines = db.log(ran)?;
        let artifacts = db.artifacts(ran)?;
        let (log_deleted, artifacts_deleted) = db.deleted(ran)?;
        Ok(Some(TaskSnapshot {
            now: now(),
            run,
            carried_from,
            log_deleted,
            artifacts_deleted,
            stage,
            task,
            lines,
            artifacts,
        }))
    };
    let wanted = |snapshot: &TaskSnapshot, event: &Event| match event {
        Event::TaskStarted { task, .. }
        | Event::TaskState { task, .. }
        | Event::TaskTests { task, .. }
        | Event::Log { task, .. }
        | Event::Artifact { task, .. } => *task == snapshot.task.id,
        Event::RunFinished { run, .. } => *run == snapshot.run.id,
        Event::RunStarted(_) | Event::Worker(_) | Event::WorkerForgotten { .. } => false,
    };
    live(boss, snapshot, wanted)
}
