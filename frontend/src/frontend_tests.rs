use crate::app::*;
use yew::prelude::*;
use yew::virtual_dom::VNode;

fn to_html(node: &Html) -> String {
    match node {
        VNode::VTag(tag) => {
            let name = tag.tag();
            let mut s = format!("<{name}");
            for (k, v) in tag.attributes.iter() {
                s.push_str(&format!(r#" {k}="{v}""#));
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

fn test_pipeline() -> PipelineDto {
    PipelineDto {
        stages: vec![
            StageDto {
                name: "build".into(),
                tasks: vec!["compile".into(), "lint".into()],
            },
            StageDto {
                name: "test".into(),
                tasks: vec!["unit".into(), "integration".into()],
            },
        ],
    }
}

fn fresh_run(pipeline: &PipelineDto) -> PipelineRunState {
    PipelineRunState {
        run_id: 0,
        active: false,
        stages: pipeline
            .stages
            .iter()
            .map(|s| StageRunState {
                name: s.name.clone(),
                tasks: s
                    .tasks
                    .iter()
                    .map(|t| TaskRunState {
                        name: t.clone(),
                        state: TaskState::Pending,
                        output: String::new(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn task(name: &str, state: TaskState) -> TaskRunState {
    TaskRunState {
        name: name.into(),
        state,
        output: String::new(),
    }
}

// -- state transition tests --

#[test]
fn toggle_theme_dark_to_light() {
    let mut theme = Theme::Dark;
    let mut run = None;
    update_state(&None, &mut run, &mut theme, Msg::ToggleTheme);
    assert_eq!(theme, Theme::Light);
}

#[test]
fn toggle_theme_light_to_dark() {
    let mut theme = Theme::Light;
    let mut run = None;
    update_state(&None, &mut run, &mut theme, Msg::ToggleTheme);
    assert_eq!(theme, Theme::Dark);
}

#[test]
fn start_run_clears_state() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));

    let result = update_state(&Some(pipeline), &mut run, &mut theme, Msg::StartRun);
    assert!(run.is_none());
    assert!(result.start_run);
    assert!(result.close_sse);
}

#[test]
fn run_created_opens_sse() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = None;

    let result = update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::RunCreated { run_id: 1 },
    );
    assert_eq!(result.open_sse, Some(1));
    assert!(run.is_none()); // state comes from SSE, not RunCreated
}

#[test]
fn run_created_without_pipeline_is_noop() {
    let mut theme = Theme::Dark;
    let mut run = None;
    let result = update_state(&None, &mut run, &mut theme, Msg::RunCreated { run_id: 1 });
    assert!(run.is_none());
    assert!(result.open_sse.is_none());
}

#[test]
fn stage_started_sets_tasks_running() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));

    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::StageStarted {
            stage_name: "build".into(),
        },
    );

    let r = run.unwrap();
    for task in &r.stages[0].tasks {
        assert_eq!(task.state, TaskState::Running);
    }
    for task in &r.stages[1].tasks {
        assert_eq!(task.state, TaskState::Pending);
    }
}

#[test]
fn stage_started_unknown_stage_is_noop() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));
    let before = run.clone();

    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::StageStarted {
            stage_name: "nonexistent".into(),
        },
    );
    assert_eq!(run, before);
}

#[test]
fn task_result_pass() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));

    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::TaskResult {
            stage_name: "build".into(),
            task_name: "compile".into(),
            passed: true,
        },
    );

    let r = run.unwrap();
    assert_eq!(r.stages[0].tasks[0].state, TaskState::Pass);
    assert_eq!(r.stages[0].tasks[1].state, TaskState::Pending);
}

#[test]
fn task_result_fail() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));

    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::TaskResult {
            stage_name: "build".into(),
            task_name: "compile".into(),
            passed: false,
        },
    );

    let r = run.unwrap();
    assert_eq!(r.stages[0].tasks[0].state, TaskState::Fail);
}

#[test]
fn task_result_unknown_task_is_noop() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));
    let before = run.clone();

    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::TaskResult {
            stage_name: "build".into(),
            task_name: "ghost".into(),
            passed: true,
        },
    );
    assert_eq!(run, before);
}

#[test]
fn run_done_signals_close_sse() {
    let mut theme = Theme::Dark;
    let mut run = None;
    let result = update_state(&None, &mut run, &mut theme, Msg::RunDone);
    assert!(result.close_sse);
}

#[test]
fn new_run_replaces_old() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = Some(fresh_run(&pipeline));

    run.as_mut().unwrap().stages[0].tasks[0].state = TaskState::Pass;

    update_state(&Some(pipeline.clone()), &mut run, &mut theme, Msg::StartRun);
    assert!(run.is_none());

    let result = update_state(
        &Some(pipeline.clone()),
        &mut run,
        &mut theme,
        Msg::RunCreated { run_id: 2 },
    );
    assert_eq!(result.open_sse, Some(2));
    assert!(run.is_none()); // state comes from SSE

    // simulate SSE state event
    update_state(
        &Some(pipeline.clone()),
        &mut run,
        &mut theme,
        Msg::SseState(fresh_run(&pipeline)),
    );
    let r = run.unwrap();
    for stage in &r.stages {
        for task in &stage.tasks {
            assert_eq!(task.state, TaskState::Pending);
        }
    }
}

#[test]
fn sse_state_sets_run() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = None;

    let state = fresh_run(&pipeline);
    update_state(
        &Some(pipeline),
        &mut run,
        &mut theme,
        Msg::SseState(state.clone()),
    );
    assert_eq!(run, Some(state));
}

#[test]
fn state_loaded_with_active_run() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = None;

    let mut state = fresh_run(&pipeline);
    state.run_id = 5;
    state.active = true;
    let result = update_state(
        &None,
        &mut run,
        &mut theme,
        Msg::StateLoaded(StateResponse {
            pipeline: pipeline.clone(),
            run: Some(state.clone()),
        }),
    );
    assert_eq!(run, Some(state));
    assert_eq!(result.open_sse, Some(5));
}

#[test]
fn state_loaded_no_run() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = None;

    let result = update_state(
        &None,
        &mut run,
        &mut theme,
        Msg::StateLoaded(StateResponse {
            pipeline,
            run: None,
        }),
    );
    assert!(run.is_none());
    assert!(result.open_sse.is_none());
}

#[test]
fn state_loaded_finished_run() {
    let pipeline = test_pipeline();
    let mut theme = Theme::Dark;
    let mut run = None;

    let mut state = fresh_run(&pipeline);
    state.run_id = 3;
    let result = update_state(
        &None,
        &mut run,
        &mut theme,
        Msg::StateLoaded(StateResponse {
            pipeline: pipeline.clone(),
            run: Some(state.clone()),
        }),
    );
    assert_eq!(run, Some(state));
    assert!(result.open_sse.is_none()); // not active, no SSE
}

// -- html rendering tests --

#[test]
fn render_task_pending() {
    assert_eq!(
        to_html(&view_task(
            &task("compile", TaskState::Pending),
            "build",
            1,
            &Theme::Dark
        )),
        r#"<li class="pending"><a href="/logs?run=1&stage=build&task=compile&theme=dark">compile</a></li>"#,
    );
}

#[test]
fn render_task_running() {
    assert_eq!(
        to_html(&view_task(
            &task("compile", TaskState::Running),
            "build",
            1,
            &Theme::Dark
        )),
        r#"<li class="running"><a href="/logs?run=1&stage=build&task=compile&theme=dark">compile</a></li>"#,
    );
}

#[test]
fn render_task_pass() {
    assert_eq!(
        to_html(&view_task(
            &task("compile", TaskState::Pass),
            "build",
            1,
            &Theme::Dark
        )),
        r#"<li class="pass"><a href="/logs?run=1&stage=build&task=compile&theme=dark">compile</a></li>"#,
    );
}

#[test]
fn render_task_fail() {
    assert_eq!(
        to_html(&view_task(
            &task("lint", TaskState::Fail),
            "build",
            1,
            &Theme::Dark
        )),
        r#"<li class="fail"><a href="/logs?run=1&stage=build&task=lint&theme=dark">lint</a></li>"#,
    );
}

#[test]
fn render_stage() {
    let stage = StageRunState {
        name: "build".into(),
        tasks: vec![
            task("compile", TaskState::Pass),
            task("lint", TaskState::Fail),
        ],
    };
    assert_eq!(
        to_html(&view_stage(&stage, 1, &Theme::Dark)),
        concat!(
            "<section>",
            "<h2>build</h2>",
            "<ul>",
            r#"<li class="pass"><a href="/logs?run=1&stage=build&task=compile&theme=dark">compile</a></li>"#,
            r#"<li class="fail"><a href="/logs?run=1&stage=build&task=lint&theme=dark">lint</a></li>"#,
            "</ul>",
            "</section>",
        ),
    );
}

#[test]
fn render_run_none_is_empty() {
    assert_eq!(to_html(&view_run(&None, &Theme::Dark)), "");
}

#[test]
fn render_run_multiple_stages() {
    let run = PipelineRunState {
        run_id: 1,
        active: false,
        stages: vec![
            StageRunState {
                name: "build".into(),
                tasks: vec![task("compile", TaskState::Pending)],
            },
            StageRunState {
                name: "test".into(),
                tasks: vec![task("unit", TaskState::Running)],
            },
        ],
    };
    assert_eq!(
        to_html(&view_run(&Some(run), &Theme::Dark)),
        concat!(
            "<section>",
            "<h2>build</h2>",
            "<ul>",
            r#"<li class="pending"><a href="/logs?run=1&stage=build&task=compile&theme=dark">compile</a></li>"#,
            "</ul>",
            "</section>",
            "<section>",
            "<h2>test</h2>",
            "<ul>",
            r#"<li class="running"><a href="/logs?run=1&stage=test&task=unit&theme=dark">unit</a></li>"#,
            "</ul>",
            "</section>",
        ),
    );
}

// -- logs page tests --

#[test]
fn render_logs_page_empty() {
    assert_eq!(
        to_html(&view_logs_page(
            "build",
            "compile",
            &Theme::Dark,
            "",
            Callback::noop()
        )),
        concat!(
            r#"<div class="dark">"#,
            "<header>",
            r#"<h1><a href="/">bettertest</a> > build > compile</h1>"#,
            r#"<div class="buttons"><button>light</button></div>"#,
            "</header>",
            r#"<pre class="logs"></pre>"#,
            "</div>",
        ),
    );
}

#[test]
fn render_logs_page_with_output() {
    assert_eq!(
        to_html(&view_logs_page(
            "build",
            "compile",
            &Theme::Dark,
            "line one\nline two",
            Callback::noop()
        )),
        concat!(
            r#"<div class="dark">"#,
            "<header>",
            r#"<h1><a href="/">bettertest</a> > build > compile</h1>"#,
            r#"<div class="buttons"><button>light</button></div>"#,
            "</header>",
            r#"<pre class="logs">line one"#,
            "\n",
            r#"line two</pre>"#,
            "</div>",
        ),
    );
}

#[test]
fn render_logs_page_light() {
    assert_eq!(
        to_html(&view_logs_page(
            "build",
            "compile",
            &Theme::Light,
            "",
            Callback::noop()
        )),
        concat!(
            r#"<div class="light">"#,
            "<header>",
            r#"<h1><a href="/">bettertest</a> > build > compile</h1>"#,
            r#"<div class="buttons"><button>dark</button></div>"#,
            "</header>",
            r#"<pre class="logs"></pre>"#,
            "</div>",
        ),
    );
}

#[test]
fn set_output_replaces_content() {
    let mut theme = Theme::Dark;
    let mut output = String::new();
    update_logs(
        &mut theme,
        &mut output,
        LogsMsg::SetOutput("hello\nworld".into()),
    );
    assert_eq!(output, "hello\nworld");
    update_logs(
        &mut theme,
        &mut output,
        LogsMsg::SetOutput("replaced".into()),
    );
    assert_eq!(output, "replaced");
}

#[test]
fn task_link_light_theme() {
    assert_eq!(
        to_html(&view_task(
            &task("compile", TaskState::Pass),
            "build",
            1,
            &Theme::Light
        )),
        r#"<li class="pass"><a href="/logs?run=1&stage=build&task=compile&theme=light">compile</a></li>"#,
    );
}
