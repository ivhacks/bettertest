// Passkeys, through webauthn-rs (Kanidm's WebAuthn library, which checks every step of the
// ceremonies with the OpenSSL compiled in). This file only fits it to the boss: which origin a
// request came to, how a user id becomes WebAuthn's user handle, and how a credential is named in
// the db.

use axum::http::{HeaderMap, header};
use uuid::Uuid;
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

/// How long a passkey ceremony may take, from the boss handing out its options to the browser
/// sending back what the authenticator made or signed.
pub const CEREMONY_MS: u64 = 5 * 60 * 1000;

/// WebAuthn for the origin the browser reached the boss at: its Host header, over HTTPS when the
/// boss speaks it. Passkeys are bound to that host name, so a passkey made at one name does not log
/// in at another.
pub fn webauthn(headers: &HeaderMap, https: bool) -> Result<Webauthn, String> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or("no Host header")?;
    let scheme = if https { "https" } else { "http" };
    let origin = Url::parse(&format!("{scheme}://{host}")).map_err(|e| e.to_string())?;
    let rp_id = origin.host_str().ok_or("no host name in the Host header")?;
    WebauthnBuilder::new(rp_id, &origin)
        .and_then(|builder| builder.rp_name("bettertest").build())
        .map_err(|e| format!("passkeys cannot work at {origin}: {e}"))
}

/// The user handle an authenticator keeps for a user, from their id.
pub fn handle(user: i64) -> Uuid {
    Uuid::from_u64_pair(0, user.cast_unsigned())
}

/// The user id a user handle is for.
pub fn user(handle: Uuid) -> i64 {
    handle.as_u64_pair().1.cast_signed()
}

/// A credential id (the authenticator's name for a passkey) as the db finds it: hex.
pub fn credential_key(id: &[u8]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "../test/passkey.rs"]
mod tests;
