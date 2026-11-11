// The users page: who may do what, and how each logs in.

use crate::{account, fetch, passkey, post_json, view};
use bettertest_shared::*;
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

pub struct UsersPage {
    users: Vec<User>,
    // Bumped by every load, to rebuild the table: a dropdown the boss refused a change for must
    // show what really is, though the data did not change.
    loads: u32,
    notice: Option<String>,
    // The user the set-password window is open for.
    password_for: Option<i64>,
    // A new API token, shown once, and whose it is.
    new_token: Option<(String, String)>,
    // A new client certificate, shown once: its name, its PKCS#12 file (base64) and its password.
    new_certificate: Option<(String, String, String)>,
    new_name: NodeRef,
    new_role: NodeRef,
    new_password: NodeRef,
    password: NodeRef,
}

pub enum UsersMsg {
    Load,
    Loaded(Vec<User>),
    Notice(String),
    Role(i64, Role),
    AskPassword(i64),
    NewToken(i64),
    TokenMade(String, String),
    NewCertificate(i64),
    CertificateMade(String, String, String),
    NewPasskey(i64),
    Revoke(i64),
    Cancel,
    SetPassword,
    Remove(i64),
    Add,
}

impl UsersPage {
    /// POSTs a change, then reloads the users, or shows why it was refused.
    fn change(ctx: &Context<Self>, url: &'static str, body: serde_json::Value) {
        let link = ctx.link().clone();
        spawn_local(async move {
            match post_json(url, &body).await {
                Ok(_) => link.send_message(UsersMsg::Load),
                Err(e) => {
                    link.send_message(UsersMsg::Notice(e));
                    // The page may show a choice the boss refused: show what really is.
                    link.send_message(UsersMsg::Load);
                }
            }
        });
    }
}

impl Component for UsersPage {
    type Message = UsersMsg;
    type Properties = ();

    // Users change rarely and only here, so the page reads them when it changes them, not live.
    fn create(ctx: &Context<Self>) -> Self {
        ctx.link().send_message(UsersMsg::Load);
        Self {
            users: vec![],
            loads: 0,
            notice: None,
            password_for: None,
            new_token: None,
            new_certificate: None,
            new_name: NodeRef::default(),
            new_role: NodeRef::default(),
            new_password: NodeRef::default(),
            password: NodeRef::default(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let value = |input: &NodeRef| {
            input
                .cast::<HtmlInputElement>()
                .map(|i| i.value())
                .unwrap_or_default()
        };
        match msg {
            UsersMsg::Load => {
                let link = ctx.link().clone();
                spawn_local(async move {
                    match fetch(Request::get("/api/users").build()).await {
                        Ok(users) => link.send_message(UsersMsg::Loaded(users)),
                        Err(e) => link.send_message(UsersMsg::Notice(e)),
                    }
                });
                return false;
            }
            UsersMsg::Loaded(users) => {
                self.users = users;
                self.loads += 1;
            }
            UsersMsg::Notice(text) => self.notice = Some(text),
            UsersMsg::Role(id, role) => {
                self.notice = None;
                Self::change(
                    ctx,
                    "/api/set-role",
                    serde_json::json!({ "id": id, "role": role }),
                );
            }
            UsersMsg::AskPassword(id) => self.password_for = Some(id),
            UsersMsg::Cancel => {
                self.password_for = None;
                self.new_token = None;
                self.new_certificate = None;
            }
            UsersMsg::NewCertificate(id) => {
                self.notice = None;
                let link = ctx.link().clone();
                spawn_local(async move {
                    let request = Request::post("/api/new-certificate")
                        .json(&serde_json::json!({ "id": id }));
                    match fetch::<serde_json::Value>(request).await {
                        Ok(made) => {
                            let field =
                                |name: &str| made[name].as_str().unwrap_or_default().to_string();
                            link.send_message(UsersMsg::CertificateMade(
                                field("label"),
                                field("p12"),
                                field("password"),
                            ));
                            link.send_message(UsersMsg::Load);
                        }
                        Err(e) => link.send_message(UsersMsg::Notice(e)),
                    }
                });
                return false;
            }
            UsersMsg::CertificateMade(label, p12, password) => {
                self.new_certificate = Some((label, p12, password));
            }
            UsersMsg::NewPasskey(id) => {
                self.notice = None;
                let link = ctx.link().clone();
                spawn_local(async move {
                    let added = passkey::ceremony(
                        "/api/new-passkey-options",
                        serde_json::json!({ "id": id }),
                        "/api/new-passkey",
                        true,
                    );
                    if let Err(e) = added.await {
                        link.send_message(UsersMsg::Notice(e));
                    }
                    link.send_message(UsersMsg::Load);
                });
                return false;
            }
            UsersMsg::Revoke(credential) => {
                Self::change(
                    ctx,
                    "/api/revoke-credential",
                    serde_json::json!({ "id": credential }),
                );
            }
            UsersMsg::NewToken(id) => {
                let name = self
                    .users
                    .iter()
                    .find(|u| u.id == id)
                    .map(|u| u.name.clone())
                    .unwrap_or_default();
                let link = ctx.link().clone();
                spawn_local(async move {
                    let request =
                        Request::post("/api/new-token").json(&serde_json::json!({ "id": id }));
                    match fetch::<serde_json::Value>(request).await {
                        Ok(made) => {
                            let token = made["token"].as_str().unwrap_or_default().to_string();
                            link.send_message(UsersMsg::TokenMade(name, token));
                            link.send_message(UsersMsg::Load);
                        }
                        Err(e) => link.send_message(UsersMsg::Notice(e)),
                    }
                });
                return false;
            }
            UsersMsg::TokenMade(name, token) => self.new_token = Some((name, token)),
            UsersMsg::SetPassword => {
                if let Some(id) = self.password_for.take() {
                    let body = serde_json::json!({ "id": id, "password": value(&self.password) });
                    Self::change(ctx, "/api/set-password", body);
                }
            }
            UsersMsg::Remove(id) => {
                Self::change(ctx, "/api/remove-user", serde_json::json!({ "id": id }))
            }
            UsersMsg::Add => {
                let role = self
                    .new_role
                    .cast::<web_sys::HtmlSelectElement>()
                    .map(|s| s.value())
                    .unwrap_or_default();
                let body = serde_json::json!({ "name": value(&self.new_name), "role": role, "password": value(&self.new_password) });
                for input in [&self.new_name, &self.new_password] {
                    if let Some(input) = input.cast::<HtmlInputElement>() {
                        input.set_value("");
                    }
                }
                self.notice = None;
                Self::change(ctx, "/api/add-user", body);
            }
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let onsubmit = link.callback(|e: SubmitEvent| {
            e.prevent_default();
            UsersMsg::Add
        });
        let add = html! {
            <form class="add" {onsubmit}>
                <input ref={self.new_name.clone()} type="text" placeholder="name" required=true />
                <select ref={self.new_role.clone()}>
                    <option value="read">{ "read" }</option>
                    <option value="write" selected=true>{ "write" }</option>
                    <option value="admin">{ "admin" }</option>
                </select>
                <input ref={self.new_password.clone()} type="password" placeholder="password, 8 or more" autocomplete="new-password" required=true />
                <button class="primary" type="submit">{ "Add User" }</button>
            </form>
        };
        let window = self.password_for.map(|id| {
            let name = self.users.iter().find(|u| u.id == id).map_or("", |u| u.name.as_str());
            let onsubmit = link.callback(|e: SubmitEvent| {
                e.prevent_default();
                UsersMsg::SetPassword
            });
            html! {
                <div class="backdrop">
                    <form class="window" role="dialog" {onsubmit}>
                        <h2>{ format!("Password for {name}") }</h2>
                        <p><input ref={self.password.clone()} type="password" placeholder="8 or more characters" autocomplete="new-password" required=true /></p>
                        <div class="buttons">
                            <button type="button" onclick={link.callback(|_| UsersMsg::Cancel)}>{ "Cancel" }</button>
                            <button class="primary" type="submit">{ "Set Password" }</button>
                        </div>
                    </form>
                </div>
            }
        });
        let token_window = self.new_token.as_ref().map(|(name, token)| {
            html! {
                <div class="backdrop">
                    <div class="window" role="dialog">
                        <h2>{ format!("A token for {name}") }</h2>
                        <p>{ "Shown this once: copy it now. Scripts and curl send it as" }</p>
                        <pre class="token">{ format!("Authorization: Bearer {token}") }</pre>
                        <div class="buttons">
                            <button class="primary" onclick={link.callback(|_| UsersMsg::Cancel)}>{ "Done" }</button>
                        </div>
                    </div>
                </div>
            }
        });
        let certificate_window = self.new_certificate.as_ref().map(|(label, p12, password)| {
            html! {
                <div class="backdrop">
                    <div class="window" role="dialog">
                        <h2>{ format!("Certificate {label}") }</h2>
                        <p>
                            <a href={format!("data:application/x-pkcs12;base64,{p12}")} download={format!("{label}.p12")}>
                                { format!("Download {label}.p12") }
                            </a>
                            { ", then import it into the browser (Settings, Certificates, Import) with this password. Both are shown this once." }
                        </p>
                        <pre class="token">{ password }</pre>
                        <div class="buttons">
                            <button class="primary" onclick={link.callback(|_| UsersMsg::Cancel)}>{ "Done" }</button>
                        </div>
                    </div>
                </div>
            }
        });
        let actions = view::UserActions {
            change: link.callback(|(id, role)| UsersMsg::Role(id, role)),
            password: link.callback(UsersMsg::AskPassword),
            token: link.callback(UsersMsg::NewToken),
            certificate: link.callback(UsersMsg::NewCertificate),
            passkey: link.callback(UsersMsg::NewPasskey),
            revoke: link.callback(UsersMsg::Revoke),
            remove: link.callback(UsersMsg::Remove),
        };
        let table = view::users(&self.users, &actions);
        html! {
            <>
                { view::header(html! { <span>{ "Users" }</span> }, account()) }
                { view::notice(&self.notice) }
                <main>
                    { add }
                    <div key={self.loads.to_string()}>{ table }</div>
                    <p class="hint">{ "To lock the boss down: add an admin with a password, then lower logged out." }</p>
                </main>
                { for window }
                { for token_window }
                { for certificate_window }
            </>
        }
    }
}
