// Tests of boss/workers.rs: the boss's connection to each worker, through stand-in workers' /events
// streams. A child of the boss's tests (see the #[path] in test/boss.rs), so it reaches what the
// boss keeps private.

use super::*;

/// A stand-in worker whose /events answers `status` with `body`, then hangs up.
async fn worker_answering(status: StatusCode, body: &'static str) -> String {
    let events = move || async move { (status, body) };
    let app = Router::new().route("/events", get(events));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

const SAYS_WHO: &str = "event: worker\ndata: {\"hostname\":\"rig\",\"arch\":\"x86_64\",\
                        \"version\":\"0.2.1\",\"labels\":[\"x86\",\"gpu\"],\"capacity\":2}\n\n";

fn info() -> WorkerInfo {
    WorkerInfo {
        hostname: "rig".into(),
        arch: Arch::X86_64,
        version: "0.2.1".into(),
        labels: vec!["x86".into(), "gpu".into()],
        capacity: 2,
    }
}

/// The boss knows `url`, and has not reached it yet.
fn known(boss: &Boss, url: &str) {
    let worker = Worker {
        url: url.into(),
        info: None,
        since: 0,
        problem: None,
        tasks: vec![],
    };
    let watched = Watched {
        worker,
        watcher: None,
    };
    lock(&boss.workers).insert(url.into(), watched);
}

fn worker(boss: &Boss, url: &str) -> Worker {
    lock(&boss.workers)[url].worker.clone()
}

#[tokio::test]
async fn following_a_worker_says_why_it_ended() {
    let boss = boss(BTreeMap::new());
    let start = now();

    let url = worker_answering(StatusCode::OK, SAYS_WHO).await;
    known(&boss, &url);
    assert_eq!(follow_worker(&boss, &url).await, format!("{url} hung up"));
    let up = worker(&boss, &url);
    assert_eq!((up.info, up.problem), (Some(info()), None));
    assert!(up.since >= start);
    // What it said was a heartbeat.
    assert!(boss.db().last_heartbeat(&url).unwrap().unwrap() >= start);

    // A worker about to restart: its runs stop and go on later, and the boss hangs up.
    let stopping = "event: worker\ndata: {\"hostname\":\"rig\",\"arch\":\"x86_64\",\"version\":\
                    \"0.2.1\",\"labels\":[\"x86\"],\"capacity\":1}\n\nevent: stopping\ndata: \
                    stopping\n\n";
    let url = worker_answering(StatusCode::OK, stopping).await;
    known(&boss, &url);
    let (cancel, cancelled) = watch::channel(None);
    let running = WorkerTask {
        id: Uuid::new_v4(),
        pipeline: "app".into(),
        run: Uuid::new_v4(),
        stage: "Build".into(),
        task: "compile".into(),
    };
    let going = Going {
        pipeline: "app".into(),
        commit: None,
        cancel,
        interrupted: false,
        spanning: Default::default(),
    };
    lock(&boss.going).insert(running.run, going);
    lock(&boss.workers)
        .get_mut(&url)
        .unwrap()
        .worker
        .tasks
        .push(running.clone());
    assert_eq!(
        follow_worker(&boss, &url).await,
        format!("{url} is restarting")
    );
    assert_eq!(*cancelled.borrow(), Some(format!("{url} is restarting")));
    assert!(lock(&boss.going)[&running.run].interrupted);

    let url = worker_answering(StatusCode::OK, "event: worker\ndata: {\"hostname\":1}\n\n").await;
    known(&boss, &url);
    let odd = follow_worker(&boss, &url).await;
    let prefix = format!("{url} said something odd about itself: ");
    assert!(odd.starts_with(&prefix), "{odd}");
    assert_eq!(worker(&boss, &url).info, None);

    let url = worker_answering(StatusCode::INTERNAL_SERVER_ERROR, "disk full").await;
    known(&boss, &url);
    assert_eq!(
        follow_worker(&boss, &url).await,
        format!("{url}/events said 500 Internal Server Error: disk full")
    );

    // Nobody listening there.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let unreachable = follow_worker(&boss, &url).await;
    let prefix = format!("could not reach {url}/events: ");
    assert!(unreachable.starts_with(&prefix), "{unreachable}");
}

/// An enrolled worker is watched: up while its stream lasts, down when it ends, since its last
/// word. Forgetting it stops the watching. Each url is enrolled once, however it is written.
#[tokio::test]
async fn an_enrolled_worker_is_watched_until_forgotten() {
    let boss = boss(BTreeMap::new());
    let url = worker_answering(StatusCode::OK, SAYS_WHO).await;
    let mut events = boss.events.subscribe();
    let add = |url: &str| add_worker(State(boss.clone()), Json(WorkerUrl { url: url.into() }));
    assert_eq!(add(&url).await, Ok(StatusCode::NO_CONTENT));
    assert_eq!(add(&format!(" {url}/ ")).await, Ok(StatusCode::NO_CONTENT));
    assert_eq!(boss.db().workers().unwrap(), [url.clone()]);
    assert_eq!(boss.worker_list().len(), 1);
    for bad in ["worker-host:7358", "ftp://worker-host", ""] {
        let refused = add(bad).await.unwrap_err();
        let why = format!("{bad:?} is not a worker url like http://worker-host:7358");
        assert_eq!(refused, bad_request(why));
    }

    // Each state it goes through, as the workers page sees it.
    let mut next_state = async || loop {
        if let Ok(Event::Worker(worker)) = events.recv().await {
            return worker;
        }
    };
    let first = timeout(Duration::from_secs(10), next_state())
        .await
        .unwrap();
    assert_eq!((first.info, first.problem), (None, None));
    let up = timeout(Duration::from_secs(10), next_state())
        .await
        .unwrap();
    assert_eq!(up.info, Some(info()));
    let down = timeout(Duration::from_secs(10), next_state())
        .await
        .unwrap();
    assert_eq!(down.info, None);
    assert_eq!(down.problem, Some(format!("{url} hung up")));
    assert_eq!(Some(down.since), boss.db().last_heartbeat(&url).unwrap());

    let forget =
        |url: &str| forget_worker(State(boss.clone()), Json(WorkerUrl { url: url.into() }));
    assert_eq!(forget(&format!("{url}/")).await, Ok(StatusCode::NO_CONTENT));
    assert!(boss.worker_list().is_empty());
    assert!(boss.db().workers().unwrap().is_empty());
    let gone = forget(&url).await.unwrap_err();
    assert_eq!(gone, not_found(format!("worker {url}")));
    // Watching stopped: nothing more about it, though it would answer again in a second.
    let quiet = timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await {
                Ok(Event::Worker(w)) => return w,
                Ok(_) => continue,
                Err(e) => panic!("{e}"),
            }
        }
    });
    assert!(quiet.await.is_err(), "a forgotten worker is still watched");
}
