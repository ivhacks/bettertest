// The workers page: every worker the boss knows, up or down, and what runs on it.

use crate::{account, act, follow, monotonic, view};
use bettertest_shared::*;
use gloo::timers::callback::Interval;
use gloo_net::http::Request;
use serde_json::json;
use yew::prelude::*;

pub struct WorkersPage {
    snapshot: Option<WorkersSnapshot>,
    // As on the task page: the boss's clock at the snapshot, and the browser's monotonic one then.
    clock: (u64, f64),
    notice: Option<String>,
    // The worker the forget window asks about, while it is open.
    forgetting: Option<String>,
    _tick: Interval,
    _stream: crate::Followed,
}

pub enum WorkersMsg {
    Live(Live<WorkersSnapshot>),
    Notice(String),
    Tick,
    Add,
    AskForget(String),
    Cancel,
    Forget,
}

impl Component for WorkersPage {
    type Message = WorkersMsg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link();
        let stream = follow(
            "/api/workers/events",
            link.callback(WorkersMsg::Live),
            link.callback(WorkersMsg::Notice),
        );
        let tick = link.callback(|()| WorkersMsg::Tick);
        Self {
            snapshot: None,
            clock: (0, 0.0),
            notice: None,
            forgetting: None,
            _tick: Interval::new(1_000, move || tick.emit(())),
            _stream: stream,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            WorkersMsg::AskForget(url) => self.forgetting = Some(url),
            WorkersMsg::Cancel => self.forgetting = None,
            WorkersMsg::Live(Live::Snapshot(snapshot)) => {
                self.clock = (snapshot.now, monotonic());
                self.snapshot = Some(snapshot);
                self.notice = None;
            }
            WorkersMsg::Live(Live::Event(event)) => {
                if let Some(snapshot) = &mut self.snapshot {
                    snapshot.apply(&event);
                }
            }
            WorkersMsg::Notice(text) => self.notice = Some(text),
            WorkersMsg::Tick => {}
            WorkersMsg::Add => {
                let Some(url) =
                    gloo::dialogs::prompt("Worker url", Some("http://worker-host:7358"))
                else {
                    return false;
                };
                // The worker shows up through the live stream like any other change.
                let request = Request::post("/api/add-worker").json(&json!({ "url": url }));
                let notice = ctx.link().callback(WorkersMsg::Notice);
                act(request, "could not add it", notice);
                return false;
            }
            WorkersMsg::Forget => {
                let Some(url) = self.forgetting.take() else {
                    return false;
                };
                let request = Request::post("/api/forget-worker").json(&json!({ "url": url }));
                let notice = ctx.link().callback(WorkersMsg::Notice);
                act(request, "could not forget it", notice);
                // The window closes now; the worker leaves through the live stream.
                return true;
            }
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let (boss_then, then) = self.clock;
        let now = boss_then + (monotonic() - then).max(0.0) as u64;
        let link = ctx.link();
        let window = match &self.forgetting {
            Some(url) => view::forget_window(
                url,
                link.callback(|_| WorkersMsg::Cancel),
                link.callback(|_| WorkersMsg::Forget),
            ),
            None => html! {},
        };
        let add = html! {
            <button class="primary" onclick={link.callback(|_| WorkersMsg::Add)}>{ "New Worker" }</button>
        };
        let body = match &self.snapshot {
            Some(snapshot) => view::workers(
                &snapshot.workers,
                now,
                &link.callback(WorkersMsg::AskForget),
            ),
            None => html! {},
        };
        html! {
            <>
                { view::header(html! {}, add, account()) }
                { view::notice(&self.notice) }
                <main>{ body }</main>
                { window }
            </>
        }
    }
}
