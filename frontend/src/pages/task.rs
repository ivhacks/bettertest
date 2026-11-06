// The task page: one task of one run, its live log, artifacts and rerun buttons.

use crate::{PageProps, account, cancel_run, follow, monotonic, view};
use bettertest_shared::*;
use gloo::timers::callback::Interval;
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

pub struct TaskPage {
    snapshot: Option<TaskSnapshot>,
    // The boss's clock when the snapshot arrived, and the browser's monotonic clock at that moment:
    // a running task's time counts up from there, whatever the browser's wall clock says.
    clock: (u64, f64),
    notice: Option<String>,
    _tick: Interval,
    _stream: crate::Followed,
}

pub enum TaskMsg {
    // Boxed: a snapshot is far bigger than a notice.
    Live(Box<Live<TaskSnapshot>>),
    Notice(String),
    Tick,
    Rerun(view::Rerun),
    Cancel(String),
}

impl Component for TaskPage {
    type Message = TaskMsg;
    type Properties = PageProps;

    fn create(ctx: &Context<Self>) -> Self {
        let url = format!("/api/task/{}/events", ctx.props().id);
        let link = ctx.link();
        let stream = follow(
            &url,
            link.callback(|live| TaskMsg::Live(Box::new(live))),
            link.callback(TaskMsg::Notice),
        );
        let tick = link.callback(|()| TaskMsg::Tick);
        Self {
            snapshot: None,
            clock: (0, 0.0),
            notice: None,
            _tick: Interval::new(1_000, move || tick.emit(())),
            _stream: stream,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            TaskMsg::Cancel(run) => {
                cancel_run(run, ctx.link().callback(TaskMsg::Notice));
                return false;
            }
            TaskMsg::Rerun(what) => {
                let Some(snapshot) = &self.snapshot else {
                    return false;
                };
                let (run, stage, task) = (
                    snapshot.run.id,
                    snapshot.stage.clone(),
                    snapshot.task.name.clone(),
                );
                let url = match what {
                    view::Rerun::Task => format!("/api/rerun/{run}/{stage}/{task}"),
                    view::Rerun::Stage => format!("/api/rerun/{run}/{stage}"),
                    view::Rerun::Run => format!("/api/rerun/{run}"),
                };
                let link = ctx.link().clone();
                // Off to this task in the new run, to watch it run again.
                spawn_local(async move {
                    let rerun = match Request::post(&url).send().await {
                        Ok(response) if response.ok() => response.json::<Run>().await,
                        Ok(response) => {
                            let body = response.text().await.unwrap_or_default();
                            link.send_message(TaskMsg::Notice(format!("could not rerun: {body}")));
                            return;
                        }
                        Err(e) => Err(e),
                    };
                    match rerun {
                        Ok(rerun) => {
                            let same = rerun
                                .stages
                                .iter()
                                .filter(|s| s.name == stage)
                                .flat_map(|s| &s.tasks)
                                .find(|t| t.name == task);
                            let page = match same {
                                Some(t) => format!("/task/{}", t.id),
                                None => format!("/pipeline/{}", rerun.pipeline),
                            };
                            crate::navigate(&page);
                        }
                        Err(e) => {
                            link.send_message(TaskMsg::Notice(format!("could not rerun: {e}")))
                        }
                    }
                });
                return false;
            }
            TaskMsg::Live(live) => match *live {
                Live::Snapshot(snapshot) => {
                    self.clock = (snapshot.now, monotonic());
                    self.snapshot = Some(snapshot);
                    self.notice = None;
                }
                Live::Event(event) => {
                    if let Some(snapshot) = &mut self.snapshot {
                        snapshot.apply(&event);
                    }
                }
            },
            TaskMsg::Notice(text) => self.notice = Some(text),
            // Only a running task's time changes.
            TaskMsg::Tick => {
                return self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.task.state == TaskState::Running);
            }
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let (boss_then, then) = self.clock;
        // Whole milliseconds are plenty, and a clock that ran backwards counts as standing still.
        let now = boss_then + (monotonic() - then).max(0.0) as u64;
        let (crumbs, right, carried, artifacts, lines) = match &self.snapshot {
            Some(snapshot) => {
                // Only a run that is over can be rerun.
                let rerun = match snapshot.run.state {
                    RunState::Running => html! {},
                    _ => view::rerun_buttons(
                        &snapshot.run,
                        &snapshot.stage,
                        &ctx.link().callback(TaskMsg::Rerun),
                    ),
                };
                let right = html! { <>{ view::task_facts(&snapshot.task, now) }{ rerun }</> };
                (
                    view::task_crumbs(snapshot, &ctx.link().callback(TaskMsg::Cancel)),
                    right,
                    html! { <>{ view::carried(snapshot) }{ view::retention_note(snapshot) }</> },
                    view::artifacts(&snapshot.task, &snapshot.artifacts),
                    view::log(&snapshot.lines),
                )
            }
            None => (html! {}, html! {}, html! {}, html! {}, html! {}),
        };
        html! {
            <>
                { view::header(crumbs, html! { <>{ right }{ account() }</> }) }
                { view::notice(&self.notice) }
                { carried }
                { artifacts }
                // column-reverse makes the browser keep the newest lines in view while the user
                // is at the bottom, and leaves them alone once they scroll up.
                <main class="follow">{ lines }</main>
            </>
        }
    }
}
