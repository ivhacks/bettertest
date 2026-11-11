// The browser's half of passkeys. The boss hands out WebAuthn options as JSON and takes back what the
// authenticator made as JSON; the browser turns one into the other (parse...FromJSON, toJSON), so
// this side only carries JSON.

use gloo_net::http::Request;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
function check() {
    if (!window.PublicKeyCredential) {
        throw "this page cannot use passkeys: browsers allow them over HTTPS, or at localhost";
    }
}

function why(e) {
    return e && e.message ? e.message : String(e);
}

export async function create_passkey(options) {
    check();
    try {
        const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(JSON.parse(options));
        const credential = await navigator.credentials.create({ publicKey });
        return JSON.stringify(credential.toJSON());
    } catch (e) {
        throw why(e);
    }
}

export async function get_passkey(options) {
    check();
    try {
        const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(JSON.parse(options));
        const credential = await navigator.credentials.get({ publicKey });
        return JSON.stringify(credential.toJSON());
    } catch (e) {
        throw why(e);
    }
}

"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn create_passkey(options: String) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn get_passkey(options: String) -> Result<JsValue, JsValue>;
}

/// Asks the boss for options (POST `options_url` with `body`), has the browser's authenticator answer
/// them (`create` makes a passkey, otherwise one is used), and POSTs that answer to `url` with the
/// ceremony's id. Returns the boss's reply, or why it failed.
pub async fn ceremony(
    options_url: &str,
    body: serde_json::Value,
    url: &str,
    create: bool,
) -> Result<String, String> {
    let options: serde_json::Value = crate::fetch(Request::post(options_url).json(&body)).await?;
    let public_key = options["publicKey"].to_string();
    let answer = match create {
        true => create_passkey(public_key).await,
        false => get_passkey(public_key).await,
    };
    let answer = answer.map_err(|e| e.as_string().unwrap_or_else(|| "passkey failed".into()))?;
    let credential: serde_json::Value =
        serde_json::from_str(&answer.as_string().unwrap_or_default()).map_err(|e| e.to_string())?;
    let finish = serde_json::json!({ "ceremony": options["ceremony"], "credential": credential });
    crate::post_json(url, &finish).await
}
