// The login page: a name and password, or a passkey.

use crate::{passkey, post_json, view};
use gloo::utils::window;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

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

/// Back to where the browser was sent to log in from, now logged in.
fn logged_in() {
    let search = window().location().search().unwrap_or_default();
    let next = search.strip_prefix("?next=").filter(|n| n.starts_with('/'));
    let _ = window().location().set_href(next.unwrap_or("/"));
}

impl Component for LoginPage {
    type Message = LoginMsg;
    type Properties = ();

    fn create(_: &Context<Self>) -> Self {
        Self {
            notice: None,
            name: NodeRef::default(),
            password: NodeRef::default(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            LoginMsg::Refused(why) => {
                self.notice = Some(why);
                true
            }
            LoginMsg::Submit => {
                let value = |input: &NodeRef| {
                    input
                        .cast::<HtmlInputElement>()
                        .map(|i| i.value())
                        .unwrap_or_default()
                };
                let body = serde_json::json!({ "name": value(&self.name), "password": value(&self.password) });
                let link = ctx.link().clone();
                spawn_local(async move {
                    match post_json("/api/login", &body).await {
                        Ok(_) => logged_in(),
                        Err(e) => link.send_message(LoginMsg::Refused(e)),
                    }
                });
                false
            }
            LoginMsg::Passkey => {
                let link = ctx.link().clone();
                spawn_local(async move {
                    let login = passkey::ceremony(
                        "/api/passkey-login-options",
                        serde_json::json!({}),
                        "/api/passkey-login",
                        false,
                    );
                    match login.await {
                        Ok(_) => logged_in(),
                        Err(e) => link.send_message(LoginMsg::Refused(e)),
                    }
                });
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let onsubmit = ctx.link().callback(|e: SubmitEvent| {
            e.prevent_default();
            LoginMsg::Submit
        });
        html! {
            <>
                { view::header(html! { <span>{ "Log In" }</span> }, html! {}) }
                { view::notice(&self.notice) }
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
