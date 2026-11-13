// Tests of view.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;
use uuid::Uuid;
use yew::virtual_dom::VNode;

// Yew has no to-string for a virtual dom outside the browser, so this renders the subset we use.
fn to_html(node: &Html) -> String {
    match node {
        VNode::VTag(tag) => {
            let name = tag.tag();
            let mut s = format!("<{name}");
            for (k, v) in tag.attributes.iter() {
                s.push_str(&format!(r#" {k}="{v}""#));
            }
            // Yew keeps an input's value apart from its attributes.
            if let Some(value) = tag.value() {
                s.push_str(&format!(r#" value="{value}""#));
            }
            s.push('>');
            if let Some(children) = tag.children() {
                s.push_str(&to_html(children));
            }
            s.push_str(&format!("</{name}>"));
            s
        }
        VNode::VText(text) => text.text.to_string(),
        VNode::VList(list) => list.iter().map(to_html).collect(),
        _ => String::new(),
    }
}

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn task(name: &str) -> Task {
    Task {
        name: name.into(),
        worker: None,
        labels: vec!["x86".into()],
        image: None,
        vm: None,
        arch: None,
        timeout: None,
        sandbox: None,
    }
}

fn pipeline() -> Pipeline {
    Pipeline {
        stages: vec![
            Stage {
                name: "Build".into(),
                sandbox: None,
                tasks: vec![task("compile")],
            },
            Stage {
                name: "Test".into(),
                sandbox: None,
                tasks: vec![task("unit"), task("e2e")],
            },
        ],
        sandboxes: Default::default(),
    }
}

#[test]
fn grid_matches_tasks_by_name() {
    // Run #1 predates the e2e task, so its e2e cell is empty.
    let runs = [Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Running,
        stages: vec![
            StageRun {
                name: "Build".into(),
                tasks: vec![TaskRun {
                    id: id(2),
                    name: "compile".into(),
                    state: TaskState::Pass,
                    worker: None,
                    started: None,
                    ended: None,
                    carried_from: None,
                    tests: None,
                }],
            },
            StageRun {
                name: "Test".into(),
                tasks: vec![TaskRun {
                    id: id(3),
                    name: "unit".into(),
                    state: TaskState::Running,
                    worker: None,
                    started: None,
                    ended: None,
                    carried_from: None,
                    tests: None,
                }],
            },
        ],
        commit: None,
    }];
    assert_eq!(
        to_html(&grid(&pipeline(), &GridLayout::default(), {
            let layout = GridLayout::default();
            let rows = runs
                .iter()
                .map(|run| run_row(&pipeline(), run, &layout, &Callback::noop()));
            html! { { for rows } }
        })),
        concat!(
            r#"<table style="grid-template-columns: 88px minmax(0, 1.0000fr) minmax(0, 0.7071fr) minmax(0, 0.7071fr)" class="grid"><thead>"#,
            r#"<tr><th></th><th style="grid-column: span 1" class="stage"><span>Build</span></th>"#,
            r#"<th style="grid-column: span 2" class="stage"><span>Test</span></th></tr>"#,
            r#"<tr><th></th><th class="task first"><span>compile</span></th><th class="task first"><span>unit</span></th>"#,
            r#"<th class="task"><span>e2e</span></th></tr></thead>"#,
            r#"<tbody><tr><th class="run running"><button title="Cancel this run: every task, now" aria-label="Cancel run" class="cancel">"#,
            r#"<svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2 2L8 8M8 2L2 8"></path></svg></button><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></th>"#,
            r#"<td class="cell pass first"><a href="/task/00000000-0000-0000-0000-000000000002" title="pass"></a></td>"#,
            r#"<td class="cell running first"><a href="/task/00000000-0000-0000-0000-000000000003" title="running"></a></td>"#,
            r#"<td class="cell"></td>"#,
            r#"</tr></tbody></table>"#,
        )
    );
}

#[test]
fn tilted_names_and_dense_stages() {
    let layout = GridLayout {
        stages: Tilt {
            angle: 90.0,
            height: 50.0,
        },
        tasks: Tilt {
            angle: 37.5,
            height: 84.0,
        },
        dense: vec![false, true],
    };
    assert_eq!(
        to_html(&grid(&pipeline(), &layout, html! {})),
        concat!(
            r#"<table style="grid-template-columns: 88px minmax(0, 1.0000fr) minmax(0, 0.7071fr) minmax(0, 0.7071fr)" class="grid"><thead>"#,
            r#"<tr class="tilted"><th style="height: 50px"></th>"#,
            r#"<th style="grid-column: span 1" class="stage"><span style="transform: rotate(-90.00deg)">Build</span></th>"#,
            r#"<th style="grid-column: span 2" class="stage"><span style="transform: rotate(-90.00deg)">Test</span></th></tr>"#,
            r#"<tr class="tilted"><th style="height: 84px"></th>"#,
            r#"<th style="height: 84px" class="task first"><span style="transform: rotate(-37.50deg)">compile</span></th>"#,
            r#"<th style="height: 84px" class="task first dense"><span style="transform: rotate(-37.50deg)">unit</span></th>"#,
            r#"<th style="height: 84px" class="task dense"><span style="transform: rotate(-37.50deg)">e2e</span></th>"#,
            r#"</tr></thead><tbody></tbody></table>"#,
        )
    );
    // The dense stage's cells name their task, since the header no longer shows it.
    let run = Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Pass,
        stages: vec![StageRun {
            name: "Test".into(),
            tasks: vec![TaskRun {
                id: id(3),
                name: "unit".into(),
                state: TaskState::Pass,
                worker: None,
                started: None,
                ended: None,
                carried_from: None,
                tests: None,
            }],
        }],
        commit: None,
    };
    assert_eq!(
        to_html(&run_row(&pipeline(), &run, &layout, &Callback::noop())),
        concat!(
            r#"<tr><th class="run pass"><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></th><td class="cell first"></td>"#,
            r#"<td class="cell pass first dense"><a href="/task/00000000-0000-0000-0000-000000000003" title="unit: pass"></a></td>"#,
            r#"<td class="cell dense"></td></tr>"#,
        )
    );
}

#[test]
fn stages_with_more_tasks_are_wider_but_each_task_narrower() {
    let one = stage_weight(1);
    let two = stage_weight(2);
    assert!(two > one);
    assert!(two / 2.0 > one / 2.0);
    assert!(two / 2.0 < one);
}

#[test]
fn names_lie_as_flat_as_they_can() {
    let all = |labels: &[f64]| labels.iter().copied().map(Some).collect::<Vec<_>>();
    // Every name fits its column: flat.
    assert_eq!(
        label_layout(&[100.0, 100.0], &all(&[60.0, 90.0]), 24.0),
        Tilt::default()
    );
    // One does not: all tilt, by the angle that puts neighbours a line apart across.
    let tilt = label_layout(&[100.0, 100.0, 300.0], &all(&[60.0, 150.0, 10.0]), 24.0);
    assert!((tilt.angle - 13.9).abs() < 0.1, "{}", tilt.angle);
    assert_eq!(tilt.height, 64.0);
    // The narrowest column sets the angle for all of them.
    let narrow = label_layout(&[100.0, 50.0, 300.0], &all(&[60.0, 150.0, 10.0]), 24.0);
    assert!((narrow.angle - 28.7).abs() < 0.1, "{}", narrow.angle);
    // Columns narrower than the spacing stand the names upright.
    let upright = label_layout(&[20.0], &all(&[150.0]), 24.0);
    assert_eq!(upright.angle, 90.0);
    // A name near the right edge tilts them all further, to stay inside the grid.
    let edge = label_layout(&[100.0, 100.0], &all(&[60.0, 150.0]), 24.0);
    assert!((edge.angle - 72.1).abs() < 0.1, "{}", edge.angle);
    // A dense stage's hidden names neither tilt the others nor need room, but leave theirs.
    let hidden = label_layout(
        &[100.0, 10.0, 300.0],
        &[Some(150.0), None, Some(10.0)],
        24.0,
    );
    assert!((hidden.angle - 13.9).abs() < 0.1, "{}", hidden.angle);
}

#[test]
fn runs_show_their_commit() {
    let run = Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Pass,
        stages: vec![],
        commit: Some(Commit {
            sha: "3fe66e3c0ffee".into(),
            branch: "main".into(),
        }),
    };
    let pipeline = Pipeline {
        stages: vec![],
        sandboxes: Default::default(),
    };
    assert_eq!(
        to_html(&run_row(
            &pipeline,
            &run,
            &GridLayout::default(),
            &Callback::noop()
        )),
        r#"<tr><th title="main @ 3fe66e3" class="run pass"><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></th></tr>"#
    );
    let snapshot = TaskSnapshot {
        now: 0,
        run,
        carried_from: None,
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Build".into(),
        task: TaskRun {
            id: id(2),
            name: "compile".into(),
            state: TaskState::Pass,
            worker: None,
            started: None,
            ended: None,
            carried_from: None,
            tests: None,
        },
        lines: vec![],
        artifacts: vec![],
    };
    assert_eq!(
        to_html(&task_crumbs(&snapshot, &Callback::noop())),
        concat!(
            r#"<a href="/pipeline/app">app</a><span class="run"><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></span>"#,
            r#"<span class="commit">main @ 3fe66e3</span><span>Build</span><span>compile</span>"#,
            r#"<span class="state pass">pass</span>"#,
        )
    );
}

#[test]
fn task_page_names_the_worker() {
    let mut task = TaskRun {
        id: id(2),
        name: "compile".into(),
        state: TaskState::Pending,
        worker: None,
        started: None,
        ended: None,
        carried_from: None,
        tests: None,
    };
    assert_eq!(to_html(&task_facts(&task, 0)), "");
    task.state = TaskState::Running;
    task.worker = Some("http://worker-a:7358".into());
    task.started = Some(1_000_000);
    assert_eq!(
        to_html(&task_facts(&task, 1_042_500)),
        concat!(
            r#"<span title="worker http://worker-a:7358" class="worker">on worker-a:7358</span>"#,
            r#"<span class="runtime">42s</span>"#,
        )
    );
}

#[test]
fn finished_tasks_keep_their_runtime() {
    let compile = TaskRun {
        id: id(2),
        name: "compile".into(),
        state: TaskState::Fail,
        worker: Some("http://worker:7358".into()),
        started: Some(1_000_000),
        ended: Some(1_083_000),
        carried_from: None,
        tests: None,
    };
    // However late the page looks.
    assert_eq!(
        to_html(&task_facts(&compile, 9_000_000)),
        concat!(
            r#"<span title="worker http://worker:7358" class="worker">on worker:7358</span>"#,
            r#"<span class="runtime">1m 23s</span>"#,
        )
    );
    let run = Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Fail,
        stages: vec![StageRun {
            name: "Build".into(),
            tasks: vec![compile],
        }],
        commit: None,
    };
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Build".into(),
            sandbox: None,
            tasks: vec![task("compile")],
        }],
        sandboxes: Default::default(),
    };
    assert_eq!(
        to_html(&run_row(
            &pipeline,
            &run,
            &GridLayout::default(),
            &Callback::noop()
        )),
        concat!(
            r#"<tr><th class="run fail"><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></th><td class="cell fail first">"#,
            r#"<a href="/task/00000000-0000-0000-0000-000000000002" title="fail in 1m 23s"></a></td></tr>"#,
        )
    );
}

#[test]
fn stages_after_a_failure_only_rerun_the_run() {
    let task = |n: u128, name: &str, state| TaskRun {
        id: id(n),
        name: name.into(),
        state,
        worker: None,
        started: None,
        ended: None,
        carried_from: None,
        tests: None,
    };
    let run = Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Fail,
        stages: vec![
            StageRun {
                name: "Build".into(),
                tasks: vec![task(2, "compile", TaskState::Pass)],
            },
            StageRun {
                name: "Test".into(),
                tasks: vec![
                    task(3, "unit", TaskState::Pass),
                    task(4, "e2e", TaskState::Fail),
                ],
            },
            StageRun {
                name: "Deploy".into(),
                tasks: vec![task(5, "push", TaskState::Skipped)],
            },
        ],
        commit: None,
    };
    let all = "<button>Rerun Task</button><button>Rerun Stage</button><button>Rerun Run</button>";
    // The stage that failed, and the ones before it, can rerun from there.
    assert_eq!(
        to_html(&rerun_buttons(&run, "Build", &Callback::noop())),
        all
    );
    assert_eq!(
        to_html(&rerun_buttons(&run, "Test", &Callback::noop())),
        all
    );
    // Deploy never ran.
    assert_eq!(
        to_html(&rerun_buttons(&run, "Deploy", &Callback::noop())),
        "<button>Rerun Run</button>"
    );
}

#[test]
fn carried_over_tasks_say_where_from() {
    let task = TaskRun {
        id: id(5),
        name: "unit".into(),
        state: TaskState::Pass,
        worker: None,
        started: None,
        ended: None,
        carried_from: Some(id(4)),
        tests: None,
    };
    let run = Run {
        id: id(1),
        pipeline: "app".into(),
        state: RunState::Pass,
        stages: vec![StageRun {
            name: "Test".into(),
            tasks: vec![task.clone()],
        }],
        commit: None,
    };
    let pipeline = Pipeline {
        stages: vec![Stage {
            name: "Test".into(),
            sandbox: None,
            tasks: vec![super::tests::task("unit")],
        }],
        sandboxes: Default::default(),
    };
    assert_eq!(
        to_html(&run_row(
            &pipeline,
            &run,
            &GridLayout::default(),
            &Callback::noop()
        )),
        concat!(
            r#"<tr><th class="run pass"><a href="/pipeline/app?run=00000000-0000-0000-0000-000000000001">000000</a></th><td class="cell pass carried first">"#,
            r#"<a href="/task/00000000-0000-0000-0000-000000000005" title="pass, carried over"></a></td></tr>"#,
        )
    );
    let snapshot = TaskSnapshot {
        now: 0,
        run,
        carried_from: Some(Uuid::from_u128(0x6dfde7 << 104)),
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Test".into(),
        task,
        lines: vec![],
        artifacts: vec![],
    };
    assert_eq!(
        to_html(&carried(&snapshot)),
        concat!(
            r#"<p class="note">Carried over from run "#,
            r#"<a href="/task/00000000-0000-0000-0000-000000000004">6dfde7</a>"#,
            r#". This rerun did not run it again.</p>"#,
        )
    );
}

#[test]
fn retention_says_what_it_deleted() {
    let task = TaskRun {
        id: id(2),
        name: "compile".into(),
        state: TaskState::Pass,
        worker: None,
        started: Some(1_000),
        ended: Some(2_000),
        carried_from: None,
        tests: None,
    };
    let mut snapshot = TaskSnapshot {
        now: 0,
        run: Run {
            id: id(1),
            pipeline: "app".into(),
            state: RunState::Pass,
            stages: vec![],
            commit: None,
        },
        carried_from: None,
        log_deleted: None,
        artifacts_deleted: None,
        stage: "Build".into(),
        task,
        lines: vec![],
        artifacts: vec![],
    };
    assert_eq!(to_html(&retention_note(&snapshot)), "");
    snapshot.log_deleted = Some(9_000);
    snapshot.artifacts_deleted = Some(9_000);
    assert_eq!(
        to_html(&retention_note(&snapshot)),
        concat!(
            r#"<p class="note">Retention deleted this task's log and artifacts. "#,
            r#"<a href="/settings">Settings</a> say how long they are kept.</p>"#,
        )
    );
    // A task that never started had no log, so there is none to have deleted.
    snapshot.task.started = None;
    snapshot.artifacts_deleted = None;
    assert_eq!(to_html(&retention_note(&snapshot)), "");
}

#[test]
fn settings_show_days_or_forever() {
    let settings = Settings {
        artifact_days: Some(30),
        log_days: None,
    };
    let refs = (NodeRef::default(), NodeRef::default());
    assert_eq!(
        to_html(&super::settings(
            &settings,
            &refs.0,
            &refs.1,
            Callback::noop()
        )),
        concat!(
            r#"<form class="settings">"#,
            r#"<label>Keep task artifacts for <input type="number" min="1" max="36500" placeholder="forever" value="30"></input> days</label>"#,
            r#"<label>Keep task logs for <input type="number" min="1" max="36500" placeholder="forever" value=""></input> days</label>"#,
            r#"<p class="hint">Counted from when each task ends. Empty keeps them forever. The boss deletes what is past its days once an hour, and as soon as you save.</p>"#,
            r#"<button type="submit" class="primary">Save</button></form>"#,
        )
    );
}

#[test]
fn durations_read_like_stopwatches() {
    assert_eq!(duration(3_456), "3.4s");
    assert_eq!(duration(42_000), "42s");
    assert_eq!(duration(83_000), "1m 23s");
    assert_eq!(duration(3_600_000 + 120_000), "1h 02m");
}

#[test]
fn log_lines_keep_their_stream() {
    let lines = vec![
        LogLine {
            stream: LogStream::Bettertest,
            line: "starting container debian".into(),
        },
        LogLine {
            stream: LogStream::Stderr,
            line: "oops".into(),
        },
    ];
    assert_eq!(
        to_html(&log(&lines)),
        r#"<pre class="log"><div class="bettertest">starting container debian</div><div class="stderr">oops</div></pre>"#
    );
}

#[test]
fn artifacts_link_to_downloads() {
    let task = TaskRun {
        id: id(7),
        name: "frame".into(),
        state: TaskState::Pass,
        worker: None,
        started: None,
        ended: None,
        carried_from: None,
        tests: None,
    };
    let artifacts = vec![
        Artifact {
            name: "frame.png".into(),
            size: 48_213,
        },
        Artifact {
            name: "notes.txt".into(),
            size: 12,
        },
    ];
    assert_eq!(
        to_html(&super::artifacts(&task, &artifacts)),
        concat!(
            r#"<nav class="artifacts">"#,
            r#"<a href="/api/task/00000000-0000-0000-0000-000000000007/artifact/frame.png" download="">frame.png<span class="size">48.2 KB</span></a>"#,
            r#"<a href="/api/task/00000000-0000-0000-0000-000000000007/artifact/notes.txt" download="">notes.txt<span class="size">12 B</span></a>"#,
            r#"</nav>"#,
        )
    );
    assert_eq!(to_html(&super::artifacts(&task, &[])), "");
}

#[test]
fn workers_show_what_runs_where() {
    let workers = vec![
        Worker {
            url: "http://worker-b:7358".into(),
            info: None,
            since: 1_000_000,
            problem: Some("could not reach http://worker-b:7358/events".into()),
            tasks: vec![],
        },
        Worker {
            url: "http://worker-a:7358".into(),
            info: Some(WorkerInfo {
                hostname: "rack-1".into(),
                arch: Arch::X86_64,
                version: "0.1.0".into(),
                labels: vec!["x86".into()],
                capacity: 2,
            }),
            since: 1_000_000,
            problem: None,
            tasks: vec![WorkerTask {
                id: id(3),
                pipeline: "app".into(),
                run: id(1),
                stage: "Test".into(),
                task: "unit".into(),
            }],
        },
    ];
    assert_eq!(
        to_html(&super::workers(&workers, 1_083_000, &Callback::noop())),
        concat!(
            r#"<table class="workers"><thead><tr><th>worker</th><th>host</th><th>arch</th><th>labels</th>"#,
            r#"<th>version</th><th>busy</th><th>for</th><th>running</th><th></th></tr></thead><tbody>"#,
            r#"<tr class="down"><td class="url">http://worker-b:7358</td>"#,
            r#"<td colspan="5" class="problem">could not reach http://worker-b:7358/events</td>"#,
            r#"<td class="since">down 1m 23s</td><td class="tasks"></td><td><button class="small">Forget</button></td></tr>"#,
            r#"<tr class="up"><td class="url">http://worker-a:7358</td>"#,
            r#"<td>rack-1</td><td>x86_64</td><td>x86</td><td>0.1.0</td><td class="busy">1/2</td><td class="since">up 1m 23s</td>"#,
            r#"<td class="tasks"><a href="/task/00000000-0000-0000-0000-000000000003">app 000000 Test.unit</a></td>"#,
            r#"<td><button class="small">Forget</button></td></tr></tbody></table>"#,
        )
    );
    assert_eq!(
        to_html(&super::workers(&[], 0, &Callback::noop())),
        r#"<p class="empty">No workers yet. A pipedef that names one adds it, or add one above.</p>"#
    );
}

#[test]
fn forgetting_a_worker_asks_first() {
    assert_eq!(
        to_html(&forget_window(
            "http://worker-03:7358",
            Callback::noop(),
            Callback::noop()
        )),
        concat!(
            r#"<div class="backdrop"><div role="dialog" class="window"><h2>Forget Worker?</h2>"#,
            r#"<p>http://worker-03:7358 stops getting tasks. A pipedef that names it will add it back.</p>"#,
            r#"<div class="buttons"><button>Cancel</button><button class="primary">Forget</button></div></div></div>"#,
        )
    );
}

#[test]
fn users_show_how_each_can_log_in() {
    let users = vec![
        User {
            id: 0,
            name: "logged out".into(),
            role: Role::Admin,
            builtin: true,
            passwords: 0,
            certificates: 0,
            passkeys: 0,
            tokens: 0,
            certs: vec![],
            keys: vec![],
        },
        User {
            id: 2,
            name: "iv".into(),
            role: Role::Write,
            builtin: false,
            passwords: 1,
            certificates: 0,
            passkeys: 1,
            tokens: 1,
            certs: vec![Certificate {
                id: 9,
                label: "iv 1".into(),
                expires: "Nov  4 12:00:00 2028 GMT".into(),
            }],
            keys: vec![Passkey {
                id: 11,
                label: "iv 1".into(),
            }],
        },
    ];
    let actions = UserActions {
        change: Callback::noop(),
        password: Callback::noop(),
        token: Callback::noop(),
        certificate: Callback::noop(),
        passkey: Callback::noop(),
        revoke: Callback::noop(),
        remove: Callback::noop(),
    };
    let html = to_html(&super::users(&users, &actions));
    assert_eq!(
        html,
        concat!(
            r#"<table class="users"><thead><tr><th>User</th><th>Role</th><th>Password</th><th>Certificate</th>"#,
            r#"<th>Passkey</th><th>API Tokens</th><th></th></tr></thead><tbody>"#,
            r#"<tr class="builtin"><td class="name">logged out</td><td><select>"#,
            r#"<option value="none">none</option><option value="read">read</option>"#,
            r#"<option value="write">write</option><option value="admin" selected="selected">admin</option></select></td>"#,
            r#"<td colspan="4" class="hint">Anyone who has not logged in</td><td class="actions"></td></tr>"#,
            r#"<tr><td class="name">iv</td><td><select>"#,
            r#"<option value="none">none</option><option value="read">read</option>"#,
            r#"<option value="write" selected="selected">write</option><option value="admin">admin</option></select></td>"#,
            r#"<td>set</td><td class="certs"><span title="expires Nov  4 12:00:00 2028 GMT" class="cert">iv 1<button class="small">Revoke</button></span></td>"#,
            r#"<td class="certs"><span class="cert">iv 1<button class="small">Revoke</button></span></td><td>1</td>"#,
            r#"<td class="actions"><button class="small">Set Password</button><button class="small">New Token</button><button class="small">New Certificate</button>"#,
            r#"<button class="small">New Passkey</button><button class="small">Remove</button></td></tr>"#,
            r#"</tbody></table>"#,
        )
    );
}

const PERSON: &str = concat!(
    r#"<svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">"#,
    r#"<circle cx="12" cy="8" r="3.5"></circle><path d="M5 20c1.3-3.3 3.7-5 7-5s5.7 1.7 7 5"></path></svg>"#,
);

#[test]
fn account_folds_into_a_person_icon_on_a_phone() {
    let me = Me {
        name: "iv".into(),
        role: Role::Write,
        logged_in: true,
    };
    assert_eq!(
        to_html(&account(&me, Callback::noop(), Callback::noop())),
        [
            r#"<button popovertarget="account" aria-label="Account, iv" class="account">"#,
            PERSON,
            r#"</button><span id="account" popover="auto" class="menu"><span class="me">iv</span>"#,
            r#"<button class="small">Add Passkey</button><button class="small">Log Out</button></span>"#,
        ]
        .concat()
    );
    let me = Me {
        name: "logged out".into(),
        role: Role::Admin,
        logged_in: false,
    };
    assert_eq!(
        to_html(&account(&me, Callback::noop(), Callback::noop())),
        [
            r#"<a href="/login" aria-label="Log In" class="page account">"#,
            PERSON,
            "<span>Log In</span></a>"
        ]
        .concat()
    );
}

#[test]
fn header_keeps_actions_together_and_links_apart() {
    assert_eq!(
        to_html(&header(
            html! { <span>{ "Workers" }</span> },
            html! { <button class="primary">{ "Add" }</button> }
        )),
        concat!(
            r#"<header><a href="/" class="home"><img src="/assets/mark.v1.svg" alt=""></img>bettertest</a>"#,
            r#"<span>Workers</span><span class="spacer"></span>"#,
            r#"<span class="actions"><button class="primary">Add</button></span>"#,
            r#"<nav class="links"><a href="/workers" class="page">Workers</a>"#,
            r#"<a href="/settings" class="page">Settings</a><a href="/users" class="page">Users</a></nav></header>"#,
        )
    );
}

#[test]
fn https_says_when_the_certificate_runs_out() {
    assert_eq!(
        to_html(&https(&None)),
        concat!(
            r#"<section class="https"><h2>HTTPS</h2>"#,
            r#"<p>Off. Start the boss with --cert and --key to turn it on.</p></section>"#,
        )
    );
    let mut on = Some(Https {
        names: vec!["ci.site.example".into(), "ci".into()],
        expires: "Mar  1 12:00:00 2027 GMT".into(),
        days_left: 119,
    });
    assert_eq!(
        to_html(&https(&on)),
        concat!(
            r#"<section class="https"><h2>HTTPS</h2>"#,
            r#"<p>On, with a certificate for ci.site.example, ci, expiring Mar  1 12:00:00 2027 GMT.</p></section>"#,
        )
    );
    if let Some(https) = &mut on {
        https.days_left = 1;
    }
    assert_eq!(
        to_html(&https(&on)),
        concat!(
            r#"<section class="https"><h2>HTTPS</h2>"#,
            r#"<p>On, with a certificate for ci.site.example, ci, expiring Mar  1 12:00:00 2027 GMT.</p>"#,
            r#"<pre class="notice">The certificate expires in 1 day. Restart the boss with a new one before then.</pre></section>"#,
        )
    );
}

#[test]
fn sizes_read_like_file_managers() {
    assert_eq!(size(999), "999 B");
    assert_eq!(size(1_000), "1.0 KB");
    assert_eq!(size(8_777_704), "8.8 MB");
    assert_eq!(size(583_729_152_000), "583.7 GB");
}

#[test]
fn tabs_mark_the_current_pipeline() {
    assert_eq!(
        to_html(&tabs(&["app".into(), "docs".into()], "docs")),
        r#"<nav class="tabs"><a href="/pipeline/app" class="tab">app</a><a href="/pipeline/docs" class="tab current">docs</a></nav>"#
    );
}

/// A results link to a run the snapshot no longer holds names the run, not the pipeline.
#[test]
fn not_recent_names_the_run() {
    assert_eq!(
        to_html(&not_recent("6dfde7")),
        r#"<pre class="notice">run 6dfde7 is not one of the 50 newest</pre>"#
    );
}

#[test]
fn results_put_failures_first_then_the_slowest() {
    let task = |n: u128, name: &str, state, seconds: u64, tests: Option<(u64, u64)>| TaskRun {
        id: id(n),
        name: name.into(),
        state,
        worker: None,
        started: (state != TaskState::Skipped).then_some(1_000_000),
        ended: (state != TaskState::Skipped).then_some(1_000_000 + seconds * 1000),
        carried_from: None,
        tests: tests.map(|(total, failed)| Tests { total, failed }),
    };
    let run = Run {
        id: id(1),
        pipeline: "sqlite".into(),
        state: RunState::Fail,
        stages: vec![
            StageRun {
                name: "Build".into(),
                tasks: vec![task(2, "testfixture", TaskState::Pass, 90, None)],
            },
            StageRun {
                name: "Test".into(),
                tasks: vec![
                    task(3, "quick", TaskState::Pass, 2, Some((1_031_284, 0))),
                    task(4, "crash", TaskState::Fail, 5, None),
                    task(5, "fts", TaskState::Fail, 1, Some((1234, 2))),
                    task(6, "busy", TaskState::Fail, 7, Some((10, 0))),
                ],
            },
            StageRun {
                name: "Report".into(),
                tasks: vec![task(7, "summary", TaskState::Skipped, 0, None)],
            },
        ],
        commit: None,
    };
    let row = |o: &str, stage: &str, n: u128, name: &str, took: &str, tests: &str, failed: &str| {
        format!(
            r#"<tr><td><span class="state {o}">{o}</span></td><td>{stage}</td><td><a href="/task/{}">{name}</a></td><td class="number">{took}</td><td class="number">{tests}</td><td class="number">{failed}</td></tr>"#,
            id(n)
        )
    };
    // Failed without a failed test (none reported, or none failed) is an error, not a failure.
    let expected = [
        concat!(
            r#"<p class="totals"><span class="state fail">1 fail</span><span class="state error">2 error</span>"#,
            r#"<span class="state skipped">1 skipped</span><span class="state pass">2 pass</span>"#,
            r#"<span>1,032,528 tests, 2 failed</span></p><table class="results">"#,
            r#"<tr><th></th><th>Stage</th><th>Task</th><th>Took</th><th>Tests</th><th>Failed</th></tr>"#,
        )
        .to_string(),
        row("fail", "Test", 5, "fts", "1.0s", "1,234", "2"),
        row("error", "Test", 6, "busy", "7.0s", "10", "0"),
        row("error", "Test", 4, "crash", "5.0s", "", ""),
        row("skipped", "Report", 7, "summary", "", "", ""),
        row("pass", "Build", 2, "testfixture", "1m 30s", "", ""),
        row("pass", "Test", 3, "quick", "2.0s", "1,031,284", "0"),
        "</table>".to_string(),
    ];
    assert_eq!(to_html(&results(&run)), expected.concat());
}

#[test]
fn counts_group_their_thousands() {
    assert_eq!(count(0), "0");
    assert_eq!(count(999), "999");
    assert_eq!(count(1000), "1,000");
    assert_eq!(count(1_031_284), "1,031,284");
}

#[test]
fn run_page_says_how_the_run_went() {
    let run = Run {
        id: id(1),
        pipeline: "sqlite".into(),
        state: RunState::Pass,
        stages: vec![],
        commit: None,
    };
    assert_eq!(
        to_html(&run_page_crumbs(&run, &Callback::noop())),
        concat!(
            r#"<a href="/pipeline/sqlite">sqlite</a><span class="run"><a href="/pipeline/sqlite?run=00000000-0000-0000-0000-000000000001">000000</a></span>"#,
            r#"<span class="state pass">pass</span>"#,
        )
    );
}
