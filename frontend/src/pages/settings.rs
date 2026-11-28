// The settings page: retention, and whether the boss speaks HTTPS.

use crate::{account, load, value, view};
use bettertest_shared::*;
use gloo_net::http::Request;
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

impl SettingsPage {
    // Settings change rarely and only here, so the page reads them when it opens, and again when it
    // comes back on screen, instead of following them. What it read before stays up meanwhile.
    fn read(ctx: &Context<Self>) {
        load(
            ctx.link(),
            Request::get("/api/settings").build(),
            SettingsMsg::Loaded,
            |e| SettingsMsg::Notice(format!("could not load them: {e}")),
        );
        load(
            ctx.link(),
            Request::get("/api/https").build(),
            SettingsMsg::Https,
            |e| SettingsMsg::Notice(format!("could not load HTTPS: {e}")),
        );
    }
}

impl Component for SettingsPage {
    type Message = SettingsMsg;
    type Properties = crate::Shown;

    fn create(ctx: &Context<Self>) -> Self {
        Self::read(ctx);
        Self {
            settings: None,
            https: None,
            saved: None,
            notice: None,
            artifacts: NodeRef::default(),
            logs: NodeRef::default(),
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, _old: &Self::Properties) -> bool {
        if ctx.props().shown {
            Self::read(ctx);
            self.saved = None;
        }
        true
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
                let days = |input: &NodeRef| value(input).trim().parse::<u32>().ok();
                let settings = Settings {
                    artifact_days: days(&self.artifacts),
                    log_days: days(&self.logs),
                };
                let request = Request::put("/api/settings").json(&settings);
                let saved = move |swept| SettingsMsg::Saved(settings, swept);
                load(ctx.link(), request, saved, SettingsMsg::Notice);
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
                { view::header(html! {}, html! {}, account()) }
                { view::notice(&self.notice) }
                <main>{ form }{ saved }{ for self.https.as_ref().map(view::https) }</main>
            </>
        }
    }
}
