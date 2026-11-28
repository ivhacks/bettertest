// The login page: a name and password, or a passkey.

use crate::{passkey, send, submit, top, value};
use gloo::utils::window;
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Default)]
pub struct LoginPage {
    notice: Option<String>,
    name: NodeRef,
    password: NodeRef,
}

pub enum LoginMsg {
    Submit,
    Passkey,
    Refused(String),
}

/// Back to where the browser was sent to log in from, now logged in. Only an app path: a `next`
/// of `//elsewhere` must not leave this boss.
async fn logged_in() {
    crate::load_me().await;
    let search = window().location().search().unwrap_or_default();
    let next = search.strip_prefix("?next=").unwrap_or("");
    let path = next.split(['?', '#']).next().unwrap_or(next);
    let next = if next.starts_with('/')
        && !next.starts_with("//")
        && path != "/login"
        && crate::is_app_path(path)
    {
        next
    } else {
        "/"
    };
    crate::navigate(next);
}

/// Logs in `how` says, then goes back to where the browser came from, or says why not.
fn log_in(
    link: &yew::html::Scope<LoginPage>,
    how: impl Future<Output = Result<String, String>> + 'static,
) {
    let link = link.clone();
    spawn_local(async move {
        match how.await {
            Ok(_) => logged_in().await,
            Err(e) => link.send_message(LoginMsg::Refused(e)),
        }
    });
}

impl Component for LoginPage {
    type Message = LoginMsg;
    type Properties = ();

    fn create(_: &Context<Self>) -> Self {
        Self::default()
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            LoginMsg::Refused(why) => {
                self.notice = Some(why);
                return true;
            }
            LoginMsg::Submit => {
                let body = serde_json::json!({ "name": value(&self.name), "password": value(&self.password) });
                log_in(ctx.link(), send(Request::post("/api/login").json(&body)));
            }
            LoginMsg::Passkey => {
                log_in(ctx.link(), passkey::ceremony(false, serde_json::json!({})))
            }
        }
        false
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let onsubmit = submit(ctx.link(), || LoginMsg::Submit);
        html! {
            <>
                { top(html! {}, html! {}, &self.notice) }
                <main>
                    <form class="login" {onsubmit}>
                        <input ref={self.name.clone()} type="text" placeholder="name" autocomplete="username" required=true />
                        <input ref={self.password.clone()} type="password" placeholder="password" autocomplete="current-password" required=true />
                        <button class="primary" type="submit">{ "Log In" }</button>
                        <button type="button" onclick={ctx.link().callback(|_| LoginMsg::Passkey)}>{ "Log In With Passkey" }</button>
                    </form>
                </main>
            </>
        }
    }
}
