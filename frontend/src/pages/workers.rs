// The workers page: every worker the boss knows, up or down, and what runs on it.

use crate::{account, follow, monotonic, post_json, view};
use bettertest_shared::*;
use gloo::timers::callback::Interval;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

pub struct WorkersPage {
    snapshot: Option<WorkersSnapshot>,
    // As on the task page: the boss's clock at the snapshot, and the browser's monotonic one then.
    clock: (u64, f64),
    notice: Option<String>,
    url: NodeRef,
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
            url: NodeRef::default(),
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
                let Some(input) = self.url.cast::<HtmlInputElement>() else {
                    return false;
                };
                let url = input.value();
                input.set_value("");
                let link = ctx.link().clone();
                // The worker shows up through the live stream like any other change.
                spawn_local(async move {
                    let body = serde_json::json!({ "url": url });
                    if let Err(e) = post_json("/api/add-worker", &body).await {
                        link.send_message(WorkersMsg::Notice(format!("could not add it: {e}")));
                    }
                });
                return false;
            }
            WorkersMsg::Forget => {
                let Some(url) = self.forgetting.take() else {
                    return false;
                };
                let link = ctx.link().clone();
                spawn_local(async move {
                    let body = serde_json::json!({ "url": url });
                    if let Err(e) = post_json("/api/forget-worker", &body).await {
                        link.send_message(WorkersMsg::Notice(format!("could not forget it: {e}")));
                    }
                });
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
        let onsubmit = link.callback(|e: SubmitEvent| {
            // The page stays; the request goes out from update().
            e.prevent_default();
            WorkersMsg::Add
        });
        let add = html! {
            <form class="add" {onsubmit}>
                <input ref={self.url.clone()} type="url" placeholder="http://worker-host:7358" required=true />
                <button class="primary" type="submit">{ "Add Worker" }</button>
            </form>
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
                { view::header(html! { <span>{ "Workers" }</span> }, html! { <>{ add }{ account() }</> }) }
                { view::notice(&self.notice) }
                <main>{ body }</main>
                { window }
            </>
        }
    }
}
