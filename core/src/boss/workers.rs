// Workers: the connection the boss keeps to each, and which one a task runs on.

use super::*;

impl Boss {
    /// The worker for task `task` of `run`: the one it names, or one with every label (see
    /// `pick_worker`), once it has a free slot. Waits while all of them are busy, and says so in the
    /// task's log. Err says why there is none, or that the run was cancelled meanwhile.
    pub(super) async fn claim_worker(
        &self,
        run: &Run,
        task: Uuid,
        named: Option<&str>,
        labels: &[String],
        cancelled: &Cancelled,
    ) -> Result<String, String> {
        let mut said = false;
        loop {
            // Listening before looking, so a slot freed in between is not missed.
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            let busy = {
                let db = self.db();
                let mut workers = lock(&self.workers);
                match pick_worker(&workers, named, labels)? {
                    Pick::Worker(worker) => {
                        self.start_task_on(&db, &mut workers, run, task, &worker);
                        return Ok(worker);
                    }
                    Pick::Busy(busy) => busy,
                }
            };
            if !said {
                self.log(task, LogStream::Bettertest, format!("waiting: {busy}"));
                said = true;
            }
            let mut cancelled = Some(cancelled.clone());
            tokio::select! {
                () = &mut freed => {}
                reason = client::cancellation(&mut cancelled) => return Err(reason),
            }
        }
    }

    /// Marks task `task` of `run` as running on `worker`, which the caller already holds: the next
    /// task of a stage that shares a sandbox, taking over the slot of the one before.
    pub(super) fn continue_on(&self, run: &Run, task: Uuid, worker: &str) {
        let db = self.db();
        let mut workers = lock(&self.workers);
        self.start_task_on(&db, &mut workers, run, task, worker);
    }

    /// Records that task `task` of `run` runs on `worker` now, under the locks the caller holds.
    pub(super) fn start_task_on(
        &self,
        db: &Db,
        workers: &mut BTreeMap<String, Watched>,
        run: &Run,
        task: Uuid,
        worker: &str,
    ) {
        let at = now();
        if let Err(e) = db.start_task(task, worker, at) {
            eprintln!("could not record task {task} as started: {e}");
        }
        // Err only means nobody is watching right now.
        let _ = self.events.send(Event::TaskStarted {
            pipeline: run.pipeline.clone(),
            run: run.id,
            task,
            worker: worker.to_string(),
            at,
        });
        let watched = workers.get_mut(worker_key(worker));
        if let (Some(watched), Some((stage, name))) = (watched, run.locate(task)) {
            watched.worker.tasks.push(WorkerTask {
                id: task,
                pipeline: run.pipeline.clone(),
                run: run.id,
                stage: stage.to_string(),
                task: name.to_string(),
            });
            let _ = self.events.send(Event::Worker(watched.worker.clone()));
        }
    }

    /// Changes what the boss knows about its workers under the lock `subscribe` takes, and
    /// broadcasts the event `change` returns, if any.
    pub(super) fn change_workers<T>(
        &self,
        change: impl FnOnce(&Db, &mut BTreeMap<String, Watched>) -> rusqlite::Result<(Option<Event>, T)>,
    ) -> rusqlite::Result<T> {
        let db = self.db();
        let mut workers = lock(&self.workers);
        let (event, value) = change(&db, &mut workers)?;
        if let Some(event) = event {
            // Err only means nobody is watching right now.
            let _ = self.events.send(event);
            self.freed.notify_waiters();
        }
        Ok(value)
    }

    /// Changes worker `url`, if the boss knows it, and tells whoever watches.
    pub(super) fn change_worker(&self, url: &str, change: impl FnOnce(&mut Worker)) {
        let changed = self.change_workers(|_, workers| {
            let Some(watched) = workers.get_mut(worker_key(url)) else {
                return Ok((None, ()));
            };
            let before = watched.worker.clone();
            change(&mut watched.worker);
            let event = (watched.worker != before).then(|| Event::Worker(watched.worker.clone()));
            Ok((event, ()))
        });
        if let Err(e) = changed {
            eprintln!("could not record a change to worker {url}: {e}");
        }
    }

    pub(super) fn worker_list(&self) -> Vec<Worker> {
        let workers = lock(&self.workers);
        workers.values().map(|w| w.worker.clone()).collect()
    }

    /// From now on, keeps a connection to the worker at `url`, unless it does already.
    pub(super) fn enroll(self: &Arc<Self>, url: &str) {
        let url = worker_key(url);
        let added = self.change_workers(|db, workers| {
            if workers.contains_key(url) {
                return Ok((None, false));
            }
            db.add_worker(url)?;
            // Down since it was last heard from, as far as the boss knows yet.
            let since = db.last_heartbeat(url)?.unwrap_or_else(now);
            let worker = Worker {
                url: url.to_string(),
                info: None,
                since,
                problem: None,
                tasks: vec![],
            };
            let watched = Watched {
                worker: worker.clone(),
                watcher: None,
            };
            workers.insert(url.to_string(), watched);
            Ok((Some(Event::Worker(worker)), true))
        });
        match added {
            Ok(true) => {
                let watcher = spawn(watch_worker(self.clone(), url.to_string())).abort_handle();
                if let Some(watched) = lock(&self.workers).get_mut(url) {
                    watched.watcher = Some(watcher);
                }
            }
            Ok(false) => {}
            Err(e) => eprintln!("could not add worker {url}: {e}"),
        }
    }

    /// Stops keeping a connection to the worker at `url`. False if the boss did not know it.
    pub(super) fn forget(&self, url: &str) -> rusqlite::Result<bool> {
        let url = worker_key(url);
        self.change_workers(|db, workers| {
            if !workers.contains_key(url) {
                return Ok((None, false));
            }
            db.forget_worker(url)?;
            if let Some(watcher) = workers.remove(url).and_then(|w| w.watcher) {
                watcher.abort();
            }
            let event = Event::WorkerForgotten {
                url: url.to_string(),
            };
            Ok((Some(event), true))
        })
    }

    /// Task `id` is over, so whichever worker it ran on no longer runs it.
    pub(super) fn task_ended(&self, id: Uuid) {
        let changed = self.change_workers(|_, workers| {
            for watched in workers.values_mut() {
                let before = watched.worker.tasks.len();
                watched.worker.tasks.retain(|t| t.id != id);
                if watched.worker.tasks.len() != before {
                    return Ok((Some(Event::Worker(watched.worker.clone())), ()));
                }
            }
            Ok((None, ()))
        });
        if let Err(e) = changed {
            eprintln!("could not record that task {id} is over: {e}");
        }
    }
}

/// A worker the boss knows, and the task that keeps a connection to it.
pub(super) struct Watched {
    pub(super) worker: Worker,
    pub(super) watcher: Option<AbortHandle>,
}

/// The longest the boss waits before trying a worker it cannot reach again.
pub(super) const MOST_PATIENT: Duration = Duration::from_secs(30);

/// How the boss names a worker: its url as pipedefs give it, without a trailing slash.
pub(super) fn worker_key(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// What picking a worker for a task came to.
pub(super) enum Pick {
    Worker(String),
    // Every worker that would do runs as many tasks as it takes; says which, for the task's log.
    Busy(String),
}

/// The worker a task (or a stage's sandbox) runs on: the one it names, or else, of the connected
/// workers with every one of `labels` and a free slot, the one running the fewest tasks (any one
/// of them at random, if several are). Err says why there is none, for the task's log.
pub(super) fn pick_worker(
    workers: &BTreeMap<String, Watched>,
    named: Option<&str>,
    labels: &[String],
) -> Result<Pick, String> {
    let full = |w: &Watched| {
        let capacity = w.worker.info.as_ref().map_or(0, |i| i.capacity as usize);
        w.worker.tasks.len() >= capacity
    };
    // The boss's first try at reaching a worker is still under way (it takes at most QUIET): until
    // it says how many tasks the worker takes, or why it cannot be reached, nothing is known.
    let connecting = |w: &Watched| w.worker.info.is_none() && w.worker.problem.is_none();
    if let Some(url) = named {
        // One the boss cannot reach is tried anyway: the task then says what went wrong.
        return Ok(match workers.get(worker_key(url)) {
            Some(w) if connecting(w) => Pick::Busy(format!("{url} is connecting")),
            Some(w) if w.worker.info.is_some() && full(w) => {
                Pick::Busy(format!("{url} is running as many tasks as it takes"))
            }
            _ => Pick::Worker(url.to_string()),
        });
    }
    let fits = |w: &&Watched| {
        let info = w.worker.info.as_ref();
        info.is_some_and(|info| labels.iter().all(|l| info.labels.contains(l)))
    };
    let mut fitting = workers.values().filter(fits).peekable();
    if fitting.peek().is_some() {
        let free: Vec<&Watched> = fitting.filter(|w| !full(w)).collect();
        let fewest = free.iter().map(|w| w.worker.tasks.len()).min();
        let least_busy: Vec<&Watched> = free
            .into_iter()
            .filter(|w| Some(w.worker.tasks.len()) == fewest)
            .collect();
        // Any of them: equally good workers share the work.
        return Ok(match fastrand::choice(least_busy) {
            Some(watched) => Pick::Worker(watched.worker.url.clone()),
            None => Pick::Busy(format!(
                "every worker with labels {} is running as many tasks as it takes",
                labels.join(", ")
            )),
        });
    }
    // A worker still connecting may be the one: the boss has just started, say.
    if workers.values().any(connecting) {
        return Ok(Pick::Busy(format!(
            "no connected worker has labels {} yet, and some are still connecting",
            labels.join(", ")
        )));
    }
    let up: Vec<String> = workers
        .values()
        .filter_map(|w| {
            let info = w.worker.info.as_ref()?;
            Some(format!("{} ({})", w.worker.url, info.labels.join(", ")))
        })
        .collect();
    let up = if up.is_empty() {
        "none".to_string()
    } else {
        up.join("; ")
    };
    // A label nobody has is most often a typo, of the pipedef's or of a worker's --label.
    let mut similar = vec![];
    for wanted in labels {
        for w in workers.values() {
            let Some(info) = &w.worker.info else { continue };
            if info.labels.contains(wanted) {
                continue;
            }
            for label in info.labels.iter().filter(|l| similar_label(wanted, l)) {
                similar.push(format!(
                    " Worker {} has the similar label {label}.",
                    w.worker.url
                ));
            }
        }
    }
    Err(format!(
        "no connected worker has labels {}.{} Connected workers: {up}. The workers page shows \
         them all.",
        labels.join(", "),
        similar.concat()
    ))
}

/// Whether label `b` looks like a typo of `a`: the same but for case, or one or two letters off
/// (one, for labels of three letters or fewer, where two would make most labels alike).
pub(super) fn similar_label(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let most = if a.chars().count().min(b.chars().count()) <= 3 {
        1
    } else {
        2
    };
    edit_distance(&a.to_lowercase(), &b.to_lowercase()) <= most
}

/// How many letters to insert, delete or change to turn `a` into `b` (Levenshtein).
pub(super) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row.first().copied().unwrap_or(0);
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
        for (j, cb) in b.iter().enumerate() {
            let above = row.get(j + 1).copied().unwrap_or(0);
            let left = row.get(j).copied().unwrap_or(0);
            let cost = usize::from(ca != *cb);
            let best = (diagonal + cost).min(above + 1).min(left + 1);
            diagonal = above;
            if let Some(cell) = row.get_mut(j + 1) {
                *cell = best;
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

/// Keeps a connection to the worker at `url` (its /events stream) for as long as the boss knows the
/// worker, and records whether it is connected. A lost connection is tried again, soon at first,
/// then less and less often.
pub(super) async fn watch_worker(boss: Shared, url: String) {
    let mut pause = Duration::from_secs(1);
    while lock(&boss.workers).contains_key(&url) {
        let problem = follow_worker(&boss, &url).await;
        // A worker that went quiet was gone from its last word, not from when the boss gave up.
        let last_heard = boss.db().last_heartbeat(&url).ok().flatten();
        let mut was_connected = false;
        boss.change_worker(&url, |w| {
            if w.info.take().is_some() {
                w.since = last_heard.unwrap_or_else(now);
                was_connected = true;
            }
            w.problem = Some(problem);
        });
        if was_connected {
            pause = Duration::from_secs(1);
        }
        sleep(pause).await;
        pause = (pause * 2).min(MOST_PATIENT);
    }
}

/// Follows the worker's /events stream for as long as it lasts, and says why it ended.
pub(super) async fn follow_worker(boss: &Boss, url: &str) -> String {
    let events = format!("{url}/events");
    // A frozen worker can accept the connection and then never answer.
    let Ok(sent) = timeout(client::QUIET, boss.http.get(&events).send()).await else {
        return format!("{url} did not answer");
    };
    let mut response = match client::check(&events, sent).await {
        Ok(response) => response,
        Err(problem) => return problem,
    };
    let mut decoder = client::SseDecoder::default();
    loop {
        let chunk = match timeout(client::QUIET, response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => return format!("{url} hung up"),
            Ok(Err(e)) => return format!("lost the connection to {url}: {}", client::why(&e)),
            Err(_) => return format!("{url} went quiet"),
        };
        // Its keep-alives come every 15 seconds: each is a heartbeat.
        if let Err(e) = boss.db().heartbeat(url, now()) {
            eprintln!("could not record a heartbeat of {url}: {e}");
        }
        for (event, data) in decoder.feed(&chunk) {
            if event != "worker" {
                continue;
            }
            match serde_json::from_str::<WorkerInfo>(&data) {
                Ok(info) => boss.change_worker(url, |w| {
                    if w.info.is_none() {
                        w.since = now();
                    }
                    w.info = Some(info);
                    w.problem = None;
                }),
                Err(e) => return format!("{url} said something odd about itself: {e}"),
            }
        }
    }
}

pub(super) async fn get_workers(State(boss): State<Shared>) -> Json<Vec<Worker>> {
    Json(boss.worker_list())
}

pub(super) async fn workers_events(State(boss): State<Shared>) -> Result<Response, Failure> {
    let workers = boss.clone();
    let snapshot = move |_: &Db| {
        Ok(Some(WorkersSnapshot {
            now: now(),
            workers: workers.worker_list(),
        }))
    };
    let wanted = |_: &WorkersSnapshot, event: &Event| {
        matches!(event, Event::Worker(_) | Event::WorkerForgotten { .. })
    };
    live(boss, snapshot, wanted)
}

#[derive(Deserialize)]
pub(super) struct WorkerUrl {
    pub(super) url: String,
}

pub(super) async fn add_worker(
    State(boss): State<Shared>,
    Json(WorkerUrl { url }): Json<WorkerUrl>,
) -> Result<StatusCode, Failure> {
    let url = url.trim();
    let valid = reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
    if !valid {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{url:?} is not a worker url like http://worker-host:7358"),
        ));
    }
    boss.enroll(url);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn forget_worker(
    State(boss): State<Shared>,
    Json(WorkerUrl { url }): Json<WorkerUrl>,
) -> Result<StatusCode, Failure> {
    if boss.forget(&url).map_err(internal)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found(format!("worker {url}")))
    }
}
