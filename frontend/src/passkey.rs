// The browser's half of passkeys. The boss hands out WebAuthn options as JSON and takes back what the
// authenticator made as JSON; the browser turns one into the other (parse...FromJSON, toJSON), so
// this side only carries JSON.

use gloo_net::http::Request;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export async function passkey(options, create) {
    if (!window.PublicKeyCredential) {
        throw "this page cannot use passkeys: browsers allow them over HTTPS, or at localhost";
    }
    try {
        const json = JSON.parse(options);
        const credential = create
            ? await navigator.credentials.create({ publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(json) })
            : await navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(json) });
        return JSON.stringify(credential.toJSON());
    } catch (e) {
        throw e && e.message ? e.message : String(e);
    }
}

"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn passkey(options: String, create: bool) -> Result<JsValue, JsValue>;
}

/// Asks the boss for options (POSTing `body`), has the browser's authenticator answer them, and
/// POSTs that answer back with the ceremony's id: `create` makes a passkey, otherwise one logs in.
/// Returns the boss's reply, or why it failed.
pub async fn ceremony(create: bool, body: serde_json::Value) -> Result<String, String> {
    let (options_url, url) = match create {
        true => ("/api/new-passkey-options", "/api/new-passkey"),
        false => ("/api/passkey-login-options", "/api/passkey-login"),
    };
    let options: serde_json::Value = crate::fetch(Request::post(options_url).json(&body)).await?;
    let public_key = options["publicKey"].to_string();
    let answer = passkey(public_key, create).await;
    let answer = answer.map_err(|e| e.as_string().unwrap_or_else(|| "passkey failed".into()))?;
    let credential: serde_json::Value =
        serde_json::from_str(&answer.as_string().unwrap_or_default()).map_err(|e| e.to_string())?;
    let finish = serde_json::json!({ "ceremony": options["ceremony"], "credential": credential });
    crate::post_json(url, &finish).await
}
