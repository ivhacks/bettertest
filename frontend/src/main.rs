mod pages;
mod passkey;
mod view;

use bettertest_shared::*;
use futures::StreamExt;
use gloo::timers::future::TimeoutFuture;
use gloo::{events::EventListener, utils::window};
use gloo_net::{eventsource::futures::EventSource, http::Request};
use serde::de::DeserializeOwned;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, wasm_bindgen::JsCast};
use yew::prelude::*;

// Every page is a full page load. The path decides which component runs. The boss redirects
// / to the first pipeline.
// Who this browser is, asked once before any page renders: the header shows it on every page.
thread_local! {
    static ME: std::cell::RefCell<Option<Me>> = const { std::cell::RefCell::new(None) };
}

fn main() {
    fade_released_buttons();
    spawn_local(async {
        let path = window().location().pathname().unwrap_or_default();
        let me = fetch::<Me>(Request::get("/api/me").build()).await.ok();
        // A boss that lets nobody in without logging in sends everyone else to log in first.
        if let Some(me) = &me
            && me.role == Role::None
            && path != "/login"
        {
            let _ = window().location().set_href(&format!("/login?next={path}"));
            return;
        }
        ME.with(|cell| *cell.borrow_mut() = me);
        render(&path);
    });
}

fn render(path: &str) {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["pipeline", name] => {
            let props = PageProps {
                id: name.to_string(),
            };
            yew::Renderer::<pages::pipeline::PipelinePage>::with_props(props).render();
        }
        ["task", id] => {
            let props = PageProps { id: id.to_string() };
            yew::Renderer::<pages::task::TaskPage>::with_props(props).render();
        }
        ["workers"] => {
            yew::Renderer::<pages::workers::WorkersPage>::new().render();
        }
        ["settings"] => {
            yew::Renderer::<pages::settings::SettingsPage>::new().render();
        }
        ["users"] => {
            yew::Renderer::<pages::users::UsersPage>::new().render();
        }
        ["login"] => {
            yew::Renderer::<pages::login::LoginPage>::new().render();
        }
        _ => {}
    }
}

/// Cancels run `run`, all of it. What happens to its tasks arrives through the live stream like
/// any other change; `notice` hears why the boss refused.
fn cancel_run(run: String, notice: Callback<String>) {
    spawn_local(async move {
        let url = format!("/api/cancel-run/{run}");
        if let Err(e) = send(Request::post(&url).build()).await {
            notice.emit(format!("could not cancel the run: {e}"));
        }
    });
}

/// The right end of every header: who is logged in, or a way to log in.
fn account() -> Html {
    let logout = Callback::from(|_: MouseEvent| {
        spawn_local(async {
            let _ = Request::post("/api/logout").send().await;
            let _ = window().location().set_href("/");
        });
    });
    // A passkey for the user logged in, made by this browser's authenticator.
    let passkey = Callback::from(|_: MouseEvent| {
        spawn_local(async {
            let added = passkey::ceremony(
                "/api/new-passkey-options",
                serde_json::json!({ "id": null }),
                "/api/new-passkey",
                true,
            )
            .await;
            let said = match added {
                Ok(reply) => {
                    let reply: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
                    let label = reply["label"].as_str().unwrap_or_default();
                    format!("Added passkey {label}. Log in with it from the login page.")
                }
                Err(e) => format!("No passkey added: {e}"),
            };
            gloo::dialogs::alert(&said);
        });
    });
    ME.with(|cell| match &*cell.borrow() {
        Some(me) => view::account(me, logout, passkey),
        None => html! {},
    })
}

/// A pressed button fades back from white once released (`@keyframes released` in style.css). CSS
/// cannot tell a release from a hover, so this marks the button the pointer went down on as
/// released when it comes up again, wherever that is. Taking the mark off at the next press lets the
/// fade start over.
fn fade_released_buttons() {
    let document = gloo::utils::document();
    let pressed: Rc<RefCell<Option<Element>>> = Rc::default();
    let down = pressed.clone();
    EventListener::new(&document, "pointerdown", move |event| {
        let target = event.target().and_then(|t| t.dyn_into::<Element>().ok());
        let button = target.and_then(|e| e.closest("button").ok().flatten());
        if let Some(button) = &button {
            let _ = button.class_list().remove_1("released");
        }
        *down.borrow_mut() = button;
    })
    .forget();
    EventListener::new(&document, "pointerup", move |_| {
        if let Some(button) = pressed.borrow_mut().take() {
            let _ = button.class_list().add_1("released");
        }
    })
    .forget();
}

#[derive(Properties, PartialEq)]
pub struct PageProps {
    pub id: String,
}

/// How long the page may hear nothing from the boss before it says so. The boss pings every 15
/// seconds; a restart, a laptop waking up or a wifi blip is over well within this, and the browser
/// reconnects by itself, so a notice for each would teach people to ignore it.
const SILENCE_MS: f64 = 60_000.0;

/// Follows a live stream from the boss: a snapshot first, then events, for as long as the page is
/// open. Hearing nothing for `SILENCE_MS`, not even a ping, shows a notice until the next snapshot
/// arrives and clears it.
fn follow<S: DeserializeOwned + 'static>(
    url: &str,
    on_live: Callback<Live<S>>,
    on_notice: Callback<String>,
) {
    let url = url.to_string();
    let heard = Rc::new(Cell::new(monotonic()));
    {
        let (heard, on_notice) = (heard.clone(), on_notice.clone());
        spawn_local(async move {
            let mut said = false;
            loop {
                TimeoutFuture::new(5_000).await;
                let silent = monotonic() - heard.get() > SILENCE_MS;
                if silent && !said {
                    on_notice.emit(
                        "the boss has not been heard from for a minute. reconnecting…".into(),
                    );
                }
                said = silent;
            }
        });
    }
    spawn_local(async move {
        loop {
            let subscribed = EventSource::new(&url).and_then(|mut source| {
                let messages = source.subscribe("message")?;
                let pings = source.subscribe("ping")?;
                Ok((source, futures::stream::select(messages, pings)))
            });
            match subscribed {
                // Dropping the EventSource closes it, so it lives while its messages are read.
                Ok((_source, mut messages)) => {
                    // The browser reconnects by itself after a dropped connection, and the boss
                    // starts the new one with a fresh snapshot. An error here means the browser
                    // gave up: the boss answered with an error, say while it restarted.
                    while let Some(Ok((event, message))) = messages.next().await {
                        heard.set(monotonic());
                        if event == "ping" {
                            continue;
                        }
                        let data = message.data().as_string().unwrap_or_default();
                        match serde_json::from_str(&data) {
                            Ok(live) => on_live.emit(live),
                            Err(e) => on_notice
                                .emit(format!("could not read a message from the boss: {e}")),
                        }
                    }
                }
                Err(e) => on_notice.emit(format!("could not follow {url}: {e}")),
            }
            // The browser gave up reconnecting, say on an error from a restarting boss. Start over;
            // the pause keeps a boss that answers with errors from being asked as fast as the
            // browser can ask.
            TimeoutFuture::new(2_000).await;
        }
    });
}

/// Milliseconds on the browser's monotonic clock.
fn monotonic() -> f64 {
    window().performance().map_or(0.0, |p| p.now())
}

/// Sends `request` to the boss and returns its reply. Err says why not, for a notice: what the boss
/// said refusing it, or what went wrong on the way.
async fn send(request: Result<Request, gloo_net::Error>) -> Result<String, String> {
    let response = request.map_err(|e| e.to_string())?.send().await;
    let response = response.map_err(|e| e.to_string())?;
    let body = response.text().await.map_err(|e| e.to_string())?;
    if response.ok() { Ok(body) } else { Err(body) }
}

/// The boss's json reply to `request`, as `T`.
async fn fetch<T: DeserializeOwned>(
    request: Result<Request, gloo_net::Error>,
) -> Result<T, String> {
    serde_json::from_str(&send(request).await?).map_err(|e| e.to_string())
}

/// POSTs `body` as json to `url`, for the reply only to say whether it worked.
async fn post_json(url: &str, body: &impl serde::Serialize) -> Result<String, String> {
    send(Request::post(url).json(body)).await
}
