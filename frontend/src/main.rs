mod pages;
mod passkey;
mod view;

use bettertest_shared::*;
use futures::StreamExt;
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
use web_sys::{Element, Event, MouseEvent, wasm_bindgen::JsCast};
use yew::prelude::*;

// The shell (this wasm, its JS, the CSS) stays loaded. Links between pages push history and swap
// the page component; they do not load the document again. The boss still serves index.html for
// every page, so a refresh or a pasted URL starts at the right place, and `/` redirects to the
// first pipeline.
// Who this browser is. The header reads it on every page.
thread_local! {
    static ME: RefCell<Option<Me>> = const { RefCell::new(None) };
    // Set once the app is up, so a page can move to another without a reload.
    static GO: RefCell<Option<Callback<String>>> = const { RefCell::new(None) };
}

fn main() {
    fade_released_buttons();
    spawn_local(async {
        let me = load_me().await;
        let path = pathname();
        // A boss that lets nobody in without logging in sends everyone else to log in first,
        // without fetching the shell a second time: it is already running.
        if me.as_ref().is_some_and(|me| me.role == Role::None) && path != "/login" {
            write_history(&format!("/login?next={path}"), true);
        }
        if let Some(root) = mount() {
            yew::Renderer::<App>::with_root(root).render();
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
    let path = path.split(['?', '#']).next().unwrap_or(path);
    matches!(path, "/" | "/workers" | "/settings" | "/users" | "/login")
        || path.starts_with("/pipeline/")
        || path.starts_with("/task/")
}

fn pathname() -> String {
    window()
        .location()
        .pathname()
        .unwrap_or_else(|_| "/".to_string())
}

fn location_search() -> String {
    window().location().search().unwrap_or_default()
}

fn write_history(url: &str, replace: bool) {
    let Ok(history) = window().history() else {
        return;
    };
    let wrote = if replace {
        history.replace_state_with_url(&JsValue::NULL, "", Some(url))
    } else {
        history.push_state_with_url(&JsValue::NULL, "", Some(url))
    };
    let _ = wrote;
}

/// Where the shell renders. The words already in the body are the first paint, before this wasm
/// ran; they go away as the app takes their place.
fn mount() -> Option<Element> {
    let body = gloo::utils::document().body()?;
    while let Some(child) = body.first_child() {
        if body.remove_child(&child).is_err() {
            break;
        }
    }
    body.dyn_into().ok()
}

enum Hist {
    Push,
    Replace,
}

enum Route {
    Home,
    Pipeline(String),
    Task(String),
    Workers,
    Settings,
    Users,
    Login,
    Missing,
}

fn route_of(path: &str) -> Route {
    let mut parts = path.trim_matches('/').split('/');
    let (first, second, third) = (parts.next().unwrap_or(""), parts.next(), parts.next());
    match (first, second, third) {
        ("", None, _) => Route::Home,
        ("pipeline", Some(name), None) if !name.is_empty() => Route::Pipeline(name.to_string()),
        ("task", Some(id), None) if !id.is_empty() => Route::Task(id.to_string()),
        ("workers", None, _) => Route::Workers,
        ("settings", None, _) => Route::Settings,
        ("users", None, _) => Route::Users,
        ("login", None, _) => Route::Login,
        _ => Route::Missing,
    }
}

fn split_url(url: &str) -> (String, String) {
    match url.split_once('?') {
        Some((path, query)) => (path.to_string(), format!("?{query}")),
        None => (url.to_string(), String::new()),
    }
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
    let target = event.target()?;
    let element = target
        .dyn_ref::<Element>()
        .cloned()
        .or_else(|| target.dyn_ref::<web_sys::Node>()?.parent_element())?;
    let anchor = element.closest("a").ok().flatten()?;
    if anchor.has_attribute("download") {
        return None;
    }
    if anchor
        .get_attribute("target")
        .is_some_and(|target| !target.is_empty() && target != "_self")
    {
        return None;
    }
    let href = anchor.get_attribute("href")?;
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let base = window().location().href().ok()?;
    let url = web_sys::Url::new_with_base(&href, &base).ok()?;
    if url.origin() != window().location().origin().ok()? {
        return None;
    }
    let path = url.pathname();
    if !is_app_path(&path) {
        return None;
    }
    mouse.prevent_default();
    Some(format!("{path}{}", url.search()))
}

struct App {
    path: String,
    search: String,
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
        let path = pathname();
        let app = Self {
            path,
            search: location_search(),
            notice: None,
            home_gen: 0,
            _pop: pop,
            _click: click,
        };
        if app.path == "/" {
            ctx.link().send_message(AppMsg::Go("/".to_string()));
        }
        app
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            AppMsg::Go(url) => {
                self.home_gen = self.home_gen.wrapping_add(1);
                let (path, search) = split_url(&url);
                if path != "/login" && locked_out() {
                    self.notice = None;
                    self.show("/login".to_string(), format!("?next={path}"), Hist::Push);
                    return true;
                }
                if path == "/" {
                    self.request_home(ctx);
                    return false;
                }
                self.notice = None;
                self.show(path, search, Hist::Push);
                true
            }
            AppMsg::Home(ticket, result) => {
                if ticket != self.home_gen {
                    return false;
                }
                match result {
                    Ok(name) => {
                        self.notice = None;
                        let mode = if self.path == "/" {
                            Hist::Replace
                        } else {
                            Hist::Push
                        };
                        self.show(format!("/pipeline/{name}"), String::new(), mode);
                    }
                    Err(text) => self.notice = Some(text),
                }
                true
            }
            AppMsg::Pop => {
                self.home_gen = self.home_gen.wrapping_add(1);
                self.path = pathname();
                self.search = location_search();
                self.notice = None;
                if self.path != "/login" && locked_out() {
                    self.show(
                        "/login".to_string(),
                        format!("?next={}", self.path),
                        Hist::Replace,
                    );
                    return true;
                }
                if self.path == "/" {
                    self.request_home(ctx);
                }
                true
            }
        }
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        html! {
            <>
                { view::notice(&self.notice) }
                { self.page() }
            </>
        }
    }
}

impl App {
    fn show(&mut self, path: String, search: String, mode: Hist) {
        if path == self.path && search == self.search {
            return;
        }
        write_history(&format!("{path}{search}"), matches!(mode, Hist::Replace));
        self.path = path;
        self.search = search;
    }

    /// `/` is the first pipeline, the same place the boss sends a browser that loads it.
    fn request_home(&mut self, ctx: &Context<Self>) {
        self.home_gen = self.home_gen.wrapping_add(1);
        let ticket = self.home_gen;
        let link = ctx.link().clone();
        spawn_local(async move {
            let found = match Request::get("/api/pipelines").send().await {
                Ok(response) => match response.json::<Vec<String>>().await {
                    Ok(names) => names
                        .into_iter()
                        .next()
                        .ok_or_else(|| "no pipelines".to_string()),
                    Err(e) => Err(e.to_string()),
                },
                Err(e) => Err(e.to_string()),
            };
            link.send_message(AppMsg::Home(ticket, found));
        });
    }

    fn page(&self) -> Html {
        // A new identity destroys the previous page, and with it the stream it was following.
        let key = format!("{}{}", self.path, self.search);
        match route_of(&self.path) {
            Route::Pipeline(name) => {
                html! { <pages::pipeline::PipelinePage key={key} id={name} /> }
            }
            Route::Task(id) => html! { <pages::task::TaskPage key={key} id={id} /> },
            Route::Workers => html! { <pages::workers::WorkersPage key={key} /> },
            Route::Settings => html! { <pages::settings::SettingsPage key={key} /> },
            Route::Users => html! { <pages::users::UsersPage key={key} /> },
            Route::Login => html! { <pages::login::LoginPage key={key} /> },
            Route::Home => html! {},
            Route::Missing => {
                html! { <pre class="notice">{ "not found" }</pre> }
            }
        }
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

/// Stops the live stream when dropped. A page holds one, so leaving the page, including by an
/// in-app navigation, hangs up instead of following a page nobody is looking at.
#[must_use = "dropping this stops the live stream"]
pub struct Followed {
    stop: Rc<Cell<bool>>,
    source: Rc<RefCell<Option<EventSource>>>,
}

impl Drop for Followed {
    fn drop(&mut self) {
        self.stop.set(true);
        // Dropping the EventSource closes it, which ends the read below.
        self.source.borrow_mut().take();
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
    let stop = Rc::new(Cell::new(false));
    let source: Rc<RefCell<Option<EventSource>>> = Rc::default();
    let heard = Rc::new(Cell::new(monotonic()));
    {
        let (heard, on_notice, stop) = (heard.clone(), on_notice.clone(), stop.clone());
        spawn_local(async move {
            let mut said = false;
            loop {
                if stop.get() {
                    return;
                }
                TimeoutFuture::new(5_000).await;
                if stop.get() {
                    return;
                }
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
    {
        let (stop, source, heard) = (stop.clone(), source.clone(), heard);
        spawn_local(async move {
            loop {
                if stop.get() {
                    return;
                }
                let subscribed = EventSource::new(&url).and_then(|mut source| {
                    let messages = source.subscribe("message")?;
                    let pings = source.subscribe("ping")?;
                    Ok((source, futures::stream::select(messages, pings)))
                });
                match subscribed {
                    Ok((event_source, mut messages)) => {
                        *source.borrow_mut() = Some(event_source);
                        // The browser reconnects by itself after a dropped connection, and the boss
                        // starts the new one with a fresh snapshot. An error here means the browser
                        // gave up: the boss answered with an error, say while it restarted.
                        while let Some(Ok((event, message))) = messages.next().await {
                            if stop.get() {
                                return;
                            }
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
                        source.borrow_mut().take();
                    }
                    Err(e) => {
                        if stop.get() {
                            return;
                        }
                        on_notice.emit(format!("could not follow {url}: {e}"));
                    }
                }
                if stop.get() {
                    return;
                }
                // The browser gave up reconnecting, say on an error from a restarting boss. Start
                // over; the pause keeps a boss that answers with errors from being asked as fast
                // as the browser can ask.
                TimeoutFuture::new(2_000).await;
            }
        });
    }
    Followed { stop, source }
}

/// Milliseconds on the browser's monotonic clock.
fn monotonic() -> f64 {
    window().performance().map_or(0.0, |p| p.now())
}

#[cfg(test)]
#[path = "../test/shell_nav.rs"]
mod shell_nav;

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
