// The settings page: retention, and whether the boss speaks HTTPS.

use crate::{account, view};
use bettertest_shared::*;
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

pub struct SettingsPage {
    settings: Option<Settings>,
    // Outer None until it loads; inner None when the boss speaks plain HTTP.
    https: Option<Option<Https>>,
    // What saving did, or what went wrong.
    saved: Option<String>,
    notice: Option<String>,
    artifacts: NodeRef,
    logs: NodeRef,
}

pub enum SettingsMsg {
    Loaded(Settings),
    Https(Option<Https>),
    Notice(String),
    Save,
    Saved(Settings, Swept),
}

/// What saving the settings deleted right away.
#[derive(serde::Deserialize)]
pub struct Swept {
    logs: usize,
    artifacts: usize,
}

impl Component for SettingsPage {
    type Message = SettingsMsg;
    type Properties = ();

    // Settings change rarely and only here, so the page reads them once instead of following them.
    fn create(ctx: &Context<Self>) -> Self {
        let link = ctx.link().clone();
        spawn_local(async move {
            let loaded = match Request::get("/api/settings").send().await {
                Ok(response) => response.json::<Settings>().await,
                Err(e) => Err(e),
            };
            match loaded {
                Ok(settings) => link.send_message(SettingsMsg::Loaded(settings)),
                Err(e) => {
                    link.send_message(SettingsMsg::Notice(format!("could not load them: {e}")))
                }
            }
        });
        let link = ctx.link().clone();
        spawn_local(async move {
            let https = match Request::get("/api/https").send().await {
                Ok(response) => response.json::<Option<Https>>().await,
                Err(e) => Err(e),
            };
            match https {
                Ok(https) => link.send_message(SettingsMsg::Https(https)),
                Err(e) => {
                    link.send_message(SettingsMsg::Notice(format!("could not load HTTPS: {e}")))
                }
            }
        });
        Self {
            settings: None,
            https: None,
            saved: None,
            notice: None,
            artifacts: NodeRef::default(),
            logs: NodeRef::default(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            SettingsMsg::Loaded(settings) => self.settings = Some(settings),
            SettingsMsg::Https(https) => self.https = Some(https),
            SettingsMsg::Notice(text) => {
                self.notice = Some(text);
                self.saved = None;
            }
            SettingsMsg::Save => {
                let days = |input: &NodeRef| {
                    let value = input.cast::<HtmlInputElement>()?.value();
                    value.trim().parse::<u32>().ok()
                };
                let settings = Settings {
                    artifact_days: days(&self.artifacts),
                    log_days: days(&self.logs),
                };
                let link = ctx.link().clone();
                spawn_local(async move {
                    let request = match Request::put("/api/settings").json(&settings) {
                        Ok(request) => request,
                        Err(e) => return link.send_message(SettingsMsg::Notice(e.to_string())),
                    };
                    let swept = match request.send().await {
                        Ok(response) if response.ok() => response.json::<Swept>().await,
                        Ok(response) => {
                            let body = response.text().await.unwrap_or_default();
                            return link.send_message(SettingsMsg::Notice(body));
                        }
                        Err(e) => Err(e),
                    };
                    match swept {
                        Ok(swept) => link.send_message(SettingsMsg::Saved(settings, swept)),
                        Err(e) => link.send_message(SettingsMsg::Notice(e.to_string())),
                    }
                });
                return false;
            }
            SettingsMsg::Saved(settings, swept) => {
                self.settings = Some(settings);
                self.notice = None;
                self.saved = Some(format!(
                    "Saved. Deleted the logs of {} tasks and the artifacts of {} just now.",
                    swept.logs, swept.artifacts
                ));
            }
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let onsubmit = ctx.link().callback(|e: SubmitEvent| {
            e.prevent_default();
            SettingsMsg::Save
        });
        let form = match &self.settings {
            Some(settings) => view::settings(settings, &self.artifacts, &self.logs, onsubmit),
            None => html! {},
        };
        let saved = match &self.saved {
            Some(text) => html! { <p class="saved">{ text }</p> },
            None => html! {},
        };
        html! {
            <>
                { view::header(html! { <span>{ "Settings" }</span> }, account()) }
                { view::notice(&self.notice) }
                <main>{ form }{ saved }{ for self.https.as_ref().map(view::https) }</main>
            </>
        }
    }
}
