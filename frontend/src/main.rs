mod pages;
mod passkey;
mod view;

use bettertest_shared::*;
use futures::{
    StreamExt,
    future::{AbortHandle, abortable, join},
};
use gloo::timers::future::TimeoutFuture;
use gloo::{
    events::{EventListener, EventListenerOptions, EventListenerPhase},
    utils::window,
};
use gloo_net::{eventsource::futures::EventSource, http::Request};
use serde::de::DeserializeOwned;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, Event, HtmlAnchorElement, MouseEvent, wasm_bindgen::JsCast};
use yew::prelude::*;

// The shell (this wasm, its JS, the CSS) stays loaded. Links between pages push history and swap
// the page component; they do not load the document again. The boss still serves index.html for
// every page, so a refresh or a pasted URL starts at the right place, and serves `/` as the first
// pipeline's page.
// Who this browser is. The header reads it on every page.
thread_local! {
    static ME: RefCell<Option<Me>> = const { RefCell::new(None) };
    // Set once the app is up, so a page can move to another without a reload.
    static GO: RefCell<Option<Callback<String>>> = const { RefCell::new(None) };
}

fn main() {
    fade_released_buttons();
    spawn_local(async {
        // The boss serves `/` as the first pipeline's page, and names it.
        let canonical = gloo::utils::document().query_selector("link[rel=canonical]");
        if let Some(page) = canonical
            .ok()
            .flatten()
            .and_then(|l| l.get_attribute("href"))
        {
            write_history(&page, true);
        }
        let me = load_me().await;
        let path = pathname();
        // A boss that lets nobody in without logging in sends everyone else to log in first,
        // without fetching the shell a second time: it is already running.
        if me.as_ref().is_some_and(|me| me.role == Role::None) && path != "/login" {
            write_history(&format!("/login?next={path}"), true);
        }
        // The words already in the body are the first paint, before this wasm ran; the app takes
        // their place.
        if let Some(body) = gloo::utils::document().body() {
            body.set_text_content(None);
            yew::Renderer::<App>::with_root(body.into()).render();
        }
    });
}

/// Asks the boss who this browser is and remembers the answer for the header.
pub async fn load_me() -> Option<Me> {
    let me = match Request::get("/api/me").send().await {
        Ok(response) => response.json::<Me>().await.ok(),
        Err(_) => None,
    };
    ME.with(|cell| *cell.borrow_mut() = me.clone());
    me
}

/// Moves to `url` (a path, and a query if it has one) inside the running shell.
pub fn navigate(url: &str) {
    let go = GO.with(|cell| cell.borrow().clone());
    if let Some(go) = go {
        go.emit(url.to_string());
    }
}

/// A page the shell draws itself. Anything else (an artifact, the API) is a real navigation.
pub fn is_app_path(path: &str) -> bool {
    page(path).is_some()
}

/// The page at `url`, keyed by it: a new key destroys the previous page, and with it the stream
/// it was following.
fn page(url: &str) -> Option<Html> {
    let key = url.to_string();
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    Some(match segments.as_slice() {
        [""] => html! {},
        ["pipeline", name] if !name.is_empty() => {
            html! { <pages::pipeline::PipelinePage key={key} id={name.to_string()} /> }
        }
        ["task", id] if !id.is_empty() => {
            html! { <pages::task::TaskPage key={key} id={id.to_string()} /> }
        }
        ["workers"] => html! { <pages::workers::WorkersPage key={key} /> },
        ["settings"] => html! { <pages::settings::SettingsPage key={key} /> },
        ["users"] => html! { <pages::users::UsersPage key={key} /> },
        ["login"] => html! { <pages::login::LoginPage key={key} /> },
        _ => return None,
    })
}

fn pathname() -> String {
    window()
        .location()
        .pathname()
        .unwrap_or_else(|_| "/".to_string())
}

/// The path and query in the address bar.
fn here() -> String {
    pathname() + &window().location().search().unwrap_or_default()
}

fn write_history(url: &str, replace: bool) {
    let Ok(history) = window().history() else {
        return;
    };
    let _ = match replace {
        true => history.replace_state_with_url(&JsValue::NULL, "", Some(url)),
        false => history.push_state_with_url(&JsValue::NULL, "", Some(url)),
    };
}

fn locked_out() -> bool {
    ME.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|me| me.role == Role::None)
    })
}

/// The link a click would open, when it is one of ours and this click would have loaded it as a
/// new document. Modified clicks keep the browser's own behavior, so a new tab still works.
fn app_link(event: &Event) -> Option<String> {
    let mouse = event.dyn_ref::<MouseEvent>()?;
    if mouse.button() != 0
        || mouse.meta_key()
        || mouse.ctrl_key()
        || mouse.shift_key()
        || mouse.alt_key()
    {
        return None;
    }
    let element = event.target()?.dyn_into::<Element>().ok()?;
    let anchor: HtmlAnchorElement = element.closest("a[href]").ok()??.dyn_into().ok()?;
    if anchor.has_attribute("download")
        || !matches!(anchor.target().as_str(), "" | "_self")
        || anchor.origin() != window().location().origin().ok()?
        || !is_app_path(&anchor.pathname())
    {
        return None;
    }
    mouse.prevent_default();
    Some(anchor.pathname() + &anchor.search())
}

struct App {
    // The page's path and query.
    url: String,
    notice: Option<String>,
    // Bumped whenever a navigation starts, so a slow answer about `/` cannot land late.
    home_gen: u32,
    _pop: EventListener,
    _click: EventListener,
}

enum AppMsg {
    Go(String),
    Home(u32, Result<String, String>),
    Pop,
}

impl Component for App {
    type Message = AppMsg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        GO.with(|cell| *cell.borrow_mut() = Some(link.callback(AppMsg::Go)));
        let pop_link = ctx.link().clone();
        let pop = EventListener::new(&window(), "popstate", move |_| {
            pop_link.send_message(AppMsg::Pop);
        });
        // The default listener is passive, and a passive listener cannot cancel the click, so the
        // browser would load the document anyway. Capture runs before anything else stops it.
        let mut options = EventListenerOptions::enable_prevent_default();
        options.phase = EventListenerPhase::Capture;
        let click =
            EventListener::new_with_options(&gloo::utils::document(), "click", options, |event| {
                if let Some(url) = app_link(event) {
                    navigate(&url);
                }
            });
        if pathname() == "/" {
            ctx.link().send_message(AppMsg::Go("/".to_string()));
        }
        Self {
            url: here(),
            notice: None,
            home_gen: 0,
            _pop: pop,
            _click: click,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            AppMsg::Go(url) => self.go(ctx, url, false),
            AppMsg::Pop => {
                self.url = here();
                self.go(ctx, self.url.clone(), true);
                true
            }
            AppMsg::Home(ticket, _) if ticket != self.home_gen => false,
            AppMsg::Home(_, Ok(name)) => {
                self.notice = None;
                let replace = self.url.split('?').next() == Some("/");
                self.show(format!("/pipeline/{name}"), replace);
                true
            }
            AppMsg::Home(_, Err(text)) => {
                self.notice = Some(text);
                true
            }
        }
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let page = page(&self.url);
        html! {
            <>
                { view::notice(&self.notice) }
                { page.unwrap_or_else(|| html! { <pre class="notice">{ "not found" }</pre> }) }
            </>
        }
    }
}

impl App {
    /// Shows `url`, or the login page to a browser that has to log in first. `/` waits for the
    /// boss to name the first pipeline, the same place it serves at `/`. Whether to render.
    fn go(&mut self, ctx: &Context<Self>, url: String, replace: bool) -> bool {
        self.home_gen = self.home_gen.wrapping_add(1);
        self.notice = None;
        let path = url.split('?').next().unwrap_or_default().to_string();
        if path != "/login" && locked_out() {
            self.show(format!("/login?next={path}"), replace);
            return true;
        }
        if path == "/" {
            self.request_home(ctx);
            return false;
        }
        self.show(url, replace);
        true
    }

    fn show(&mut self, url: String, replace: bool) {
        if url != self.url {
            write_history(&url, replace);
            self.url = url;
        }
    }

    fn request_home(&self, ctx: &Context<Self>) {
        let ticket = self.home_gen;
        let link = ctx.link().clone();
        spawn_local(async move {
            let names = fetch::<Vec<String>>(Request::get("/api/pipelines").build()).await;
            let first =
                names.and_then(|names| names.into_iter().next().ok_or("no pipelines".into()));
            link.send_message(AppMsg::Home(ticket, first));
        });
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
            load_me().await;
            navigate("/");
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

/// A pressed button, or grid cell, fades back from white once released (`@keyframes released` in
/// style.css). CSS cannot tell a release from a hover, so this marks the button the pointer went
/// down on as released when it comes up again, wherever that is. Taking the mark off at the next
/// press lets the fade start over.
fn fade_released_buttons() {
    let document = gloo::utils::document();
    let pressed: Rc<RefCell<Option<Element>>> = Rc::default();
    let down = pressed.clone();
    EventListener::new(&document, "pointerdown", move |event| {
        let target = event.target().and_then(|t| t.dyn_into::<Element>().ok());
        let button = target.and_then(|e| e.closest("button, .cell a").ok().flatten());
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

/// Stops the live stream when dropped. A page holds one, so leaving the page, including by an
/// in-app navigation, hangs up instead of following a page nobody is looking at.
#[must_use = "dropping this stops the live stream"]
pub struct Followed(AbortHandle);

impl Drop for Followed {
    fn drop(&mut self) {
        // Drops what follows the stream, and with it the EventSource, which closes it.
        self.0.abort();
    }
}

/// Follows a live stream from the boss: a snapshot first, then events, until `Followed` is
/// dropped. Hearing nothing for `SILENCE_MS`, not even a ping, shows a notice until the next
/// snapshot arrives and clears it.
fn follow<S: DeserializeOwned + 'static>(
    url: &str,
    on_live: Callback<Live<S>>,
    on_notice: Callback<String>,
) -> Followed {
    let url = url.to_string();
    let heard = Rc::new(Cell::new(monotonic()));
    let watch = {
        let (heard, on_notice) = (heard.clone(), on_notice.clone());
        async move {
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
        }
    };
    let read = async move {
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
    };
    let (following, stop) = abortable(join(watch, read));
    spawn_local(async move {
        let _ = following.await;
    });
    Followed(stop)
}

/// Milliseconds on the browser's monotonic clock.
fn monotonic() -> f64 {
    window().performance().map_or(0.0, |p| p.now())
}

#[cfg(test)]
#[path = "../test/shell_nav.rs"]
mod shell_nav;

#[cfg(test)]
#[path = "../test/pipeline_page.rs"]
mod pipeline_page;

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
