// Users: who a request is, logging in (passwords, certificates, passkeys), and the users page.

use super::*;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

impl Boss {
    /// Keeps a passkey ceremony's state for five minutes, under a new id the browser sends back.
    pub(super) fn begin_ceremony(&self, ceremony: Ceremony) -> Result<String, Failure> {
        let (id, _) = auth::new_token().map_err(internal)?;
        let now = now();
        let mut ceremonies = lock(&self.ceremonies);
        ceremonies.retain(|_, p| p.expires > now);
        if ceremonies.len() >= MOST_CEREMONIES {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "too many passkey ceremonies under way: try again in a few minutes".into(),
            ));
        }
        let expires = now + passkey::CEREMONY_MS;
        ceremonies.insert(id.clone(), Pending { ceremony, expires });
        Ok(id)
    }

    /// Ends a ceremony, which works once, handing back its state.
    pub(super) fn end_ceremony(&self, id: &str) -> Result<Ceremony, Failure> {
        match lock(&self.ceremonies).remove(id) {
            Some(pending) if pending.expires > now() => Ok(pending.ceremony),
            _ => Err((
                StatusCode::UNAUTHORIZED,
                "that passkey ceremony is unknown, finished or expired: try again".into(),
            )),
        }
    }

    /// WebAuthn for the origin this request came to.
    pub(super) fn webauthn(
        &self,
        headers: &HeaderMap,
    ) -> Result<webauthn_rs::prelude::Webauthn, Failure> {
        passkey::webauthn(headers, self.tls.is_some()).map_err(|e| (StatusCode::BAD_REQUEST, e))
    }
}

/// A passkey ceremony under way: adding a passkey to a user, or logging in with one. webauthn-rs's
/// state for it holds the challenge the authenticator has to sign.
pub(super) enum Ceremony {
    Register {
        user: i64,
        state: PasskeyRegistration,
    },
    Login(DiscoverableAuthentication),
}

pub(super) struct Pending {
    pub(super) ceremony: Ceremony,
    pub(super) expires: u64,
}

/// The most passkey ceremonies under way at once. Each is a few dozen bytes for five minutes, and
/// anyone may start a login, so this bounds what they can make the boss hold.
pub(super) const MOST_CEREMONIES: usize = 10_000;

/// Who the request is: its session's user, or the logged-out user. Its role is the higher of the two,
/// so logging in never takes anything away. Extends a session that is in use.
pub(super) fn whoami(
    boss: &Boss,
    headers: &HeaderMap,
    peer: Option<&tls::Peer>,
) -> Result<Me, Failure> {
    who(boss, headers, peer).map(|(me, _)| me)
}

/// whoami, and the id of its user when it is one (not logged out, not a running task).
pub(super) fn who(
    boss: &Boss,
    headers: &HeaderMap,
    peer: Option<&tls::Peer>,
) -> Result<(Me, Option<i64>), Failure> {
    let db = boss.db();
    let users = db.users().map_err(internal)?;
    let logged_out = users
        .iter()
        .find(|u| u.builtin)
        .map_or(Role::None, |u| u.role);
    let cookies = text_header(headers, header::COOKIE);
    let now = now();
    let authorization = text_header(headers, header::AUTHORIZATION);
    // Logging in never takes away what logged out may do.
    let me = |name, role: Role| Me {
        name,
        role: role.max(logged_out),
        logged_in: true,
    };
    if let Some(token) = auth::bearer(authorization) {
        let hash = auth::token_hash(token);
        // A running task reads what its run needs; nothing more.
        if lock(&boss.task_tokens).contains_key(&hash) {
            return Ok((me("a running task".into(), Role::Read), None));
        }
        if let Some((id, name, role)) = db.api_token(&hash).map_err(internal)? {
            return Ok((me(name, role), Some(id)));
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            "that token is not, or no longer, valid".into(),
        ));
    }
    if let Some(token) = auth::session_token(cookies) {
        let hash = auth::token_hash(token);
        if let Some((id, name, role, expires)) = db.session(&hash, now).map_err(internal)? {
            // Pushed out once a day at most, so using the boss does not write on every request.
            if expires < now + auth::SESSION_MS - 24 * 60 * 60 * 1000 {
                db.extend_session(&hash, now + auth::SESSION_MS)
                    .map_err(internal)?;
            }
            return Ok((me(name, role), Some(id)));
        }
    }
    // A client certificate the browser presented over HTTPS logs its user in on every request.
    if let Some(fingerprint) = peer.and_then(|p| p.certificate.as_deref())
        && let Some((id, name, role)) = db.certificate_user(fingerprint).map_err(internal)?
    {
        return Ok((me(name, role), Some(id)));
    }
    let logged_out = Me {
        name: "logged out".into(),
        role: logged_out,
        logged_in: false,
    };
    Ok((logged_out, None))
}

/// Header `name` as text, or "" without one.
fn text_header(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Lets a request through if who sent it may do what it asks (see auth::needed). Otherwise 401 or
/// 403, with what to do about it.
pub(super) async fn guard(
    State(boss): State<Shared>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(needed) = auth::needed(request.method().as_str(), request.uri().path()) else {
        return next.run(request).await;
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<tls::Peer>>()
        .map(|c| &c.0);
    let me = match whoami(&boss, request.headers(), peer) {
        Ok(me) => me,
        Err(failure) => return failure.into_response(),
    };
    if me.role >= needed {
        return next.run(request).await;
    }
    // 401 when logging in could help, 403 when the user is logged in and still may not.
    match me.logged_in {
        false => {
            let why = format!("log in to do this: it needs {} access", needed.as_str());
            (StatusCode::UNAUTHORIZED, why).into_response()
        }
        true => {
            let why = format!(
                "{} has {} access, and this needs {}",
                me.name,
                me.role.as_str(),
                needed.as_str()
            );
            (StatusCode::FORBIDDEN, why).into_response()
        }
    }
}

pub(super) async fn get_me(
    State(boss): State<Shared>,
    ConnectInfo(peer): ConnectInfo<tls::Peer>,
    headers: HeaderMap,
) -> Result<Json<Me>, Failure> {
    whoami(&boss, &headers, Some(&peer)).map(Json)
}

#[derive(Deserialize)]
pub(super) struct Login {
    pub(super) name: String,
    pub(super) password: String,
}

/// Starts a session for a user whose password checks out, in an HttpOnly cookie.
pub(super) async fn login(
    State(boss): State<Shared>,
    Json(login): Json<Login>,
) -> Result<Response, Failure> {
    let found = boss.db().password(&login.name).map_err(internal)?;
    // The same answer for an unknown name as for a wrong password, so it says nothing about who exists.
    let refused = (
        StatusCode::UNAUTHORIZED,
        "that name and password do not match".to_string(),
    );
    let (user, stored) = found.ok_or_else(|| refused.clone())?;
    // Argon2 takes a moment of CPU and memory on purpose: off the async threads.
    let password = login.password;
    let checks = tokio::task::spawn_blocking(move || auth::check_password(&password, &stored));
    if !checks.await.map_err(internal)? {
        return Err(refused);
    }
    start_session(&boss, user)
}

/// Logs the browser in as `user`: a new session, in an HttpOnly cookie.
pub(super) fn start_session(boss: &Boss, user: i64) -> Result<Response, Failure> {
    let (token, hash) = auth::new_token().map_err(internal)?;
    boss.db()
        .add_session(&hash, user, now() + auth::SESSION_MS)
        .map_err(internal)?;
    let max_age = auth::SESSION_MS / 1000;
    let cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}",
        auth::COOKIE
    );
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

pub(super) async fn logout(
    State(boss): State<Shared>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    if let Some(token) = auth::session_token(text_header(&headers, header::COOKIE)) {
        boss.db()
            .end_session(&auth::token_hash(token), now())
            .map_err(internal)?;
    }
    let cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        auth::COOKIE
    );
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

pub(super) async fn get_users(State(boss): State<Shared>) -> Result<Json<Vec<User>>, Failure> {
    boss.db().users().map(Json).map_err(internal)
}

#[derive(Deserialize)]
pub(super) struct NewUser {
    pub(super) name: String,
    pub(super) role: Role,
    pub(super) password: String,
}

pub(super) async fn add_user(
    State(boss): State<Shared>,
    Json(new): Json<NewUser>,
) -> Result<StatusCode, Failure> {
    let name = new.name.trim();
    if name.is_empty() || name == "logged out" {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{name:?} cannot be a user's name"),
        ));
    }
    let hash = hash_password(new.password).await?;
    let mut db = boss.db();
    let id = db.add_user(name, new.role).map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            (
                StatusCode::CONFLICT,
                format!("there is a user named {name} already"),
            )
        }
        e => internal(e),
    })?;
    db.set_password(id, &hash).map_err(internal)?;
    Ok(StatusCode::CREATED)
}

/// A new password's hash, once it is long enough. Argon2 takes a moment of CPU and memory on
/// purpose: off the async threads.
async fn hash_password(password: String) -> Result<String, Failure> {
    if password.chars().count() < 8 {
        let short = "a password has at least 8 characters";
        return Err((StatusCode::BAD_REQUEST, short.into()));
    }
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password));
    hash.await.map_err(internal)?.map_err(internal)
}

/// `name` and a number past every one in `labels`, so no two of a user's certificates or
/// passkeys share a name, revoked or not.
fn next_label<'a>(name: &str, labels: impl Iterator<Item = &'a String>) -> String {
    let numbers = labels.filter_map(|label| label.rsplit(' ').next()?.parse::<u32>().ok());
    format!("{name} {}", numbers.max().unwrap_or(0) + 1)
}

/// Refuses a change to users that would leave nobody able to administer the boss.
pub(super) fn check_lockout(
    boss: &Boss,
    change: impl FnOnce(&mut Vec<User>),
) -> Result<(), Failure> {
    let mut users = boss.db().users().map_err(internal)?;
    change(&mut users);
    if auth::locks_out(&users) {
        return Err((
            StatusCode::CONFLICT,
            "that would leave nobody able to administer bettertest: add an admin who can log in \
             first, or keep logged out at admin"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct SetRole {
    pub(super) id: i64,
    pub(super) role: Role,
}

pub(super) async fn set_role(
    State(boss): State<Shared>,
    Json(set): Json<SetRole>,
) -> Result<StatusCode, Failure> {
    check_lockout(&boss, |users| {
        for u in users.iter_mut().filter(|u| u.id == set.id) {
            u.role = set.role;
        }
    })?;
    boss.db().set_role(set.id, set.role).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct SetPassword {
    pub(super) id: i64,
    pub(super) password: String,
}

pub(super) async fn set_password(
    State(boss): State<Shared>,
    Json(set): Json<SetPassword>,
) -> Result<StatusCode, Failure> {
    if set.id == 0 {
        return Err((StatusCode::BAD_REQUEST, "logged out has no password".into()));
    }
    let hash = hash_password(set.password).await?;
    boss.db().set_password(set.id, &hash).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct UserId {
    pub(super) id: i64,
}

pub(super) async fn remove_user(
    State(boss): State<Shared>,
    Json(user): Json<UserId>,
) -> Result<StatusCode, Failure> {
    if user.id == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "logged out is always there; lower its role instead".into(),
        ));
    }
    check_lockout(&boss, |users| users.retain(|u| u.id != user.id))?;
    boss.db().remove_user(user.id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(super) struct NewToken {
    pub(super) token: String,
}

/// Makes an API token for a user, for scripts and curl. It is shown this once: the db keeps only
/// its hash.
pub(super) async fn new_token(
    State(boss): State<Shared>,
    Json(user): Json<UserId>,
) -> Result<Json<NewToken>, Failure> {
    if user.id == 0 {
        return Err((StatusCode::BAD_REQUEST, "logged out needs no token".into()));
    }
    let (token, hash) = auth::new_token().map_err(internal)?;
    boss.db().add_api_token(&hash, user.id).map_err(internal)?;
    Ok(Json(NewToken { token }))
}

pub(super) async fn revoke_tokens(
    State(boss): State<Shared>,
    Json(user): Json<UserId>,
) -> Result<StatusCode, Failure> {
    boss.db().revoke_api_tokens(user.id).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(super) struct NewCertificate {
    pub(super) label: String,
    // The PKCS#12 file, base64, and its password: shown this once.
    pub(super) p12: String,
    pub(super) password: String,
}

/// Makes a client certificate for a user, for certificate login: the browser presents it over
/// HTTPS and is that user. The boss keeps only its fingerprint; key and file leave with this answer.
pub(super) async fn new_certificate(
    State(boss): State<Shared>,
    Json(user): Json<UserId>,
) -> Result<Json<NewCertificate>, Failure> {
    if boss.tls.is_none() {
        return Err((
            StatusCode::CONFLICT,
            "certificate login needs HTTPS: start the boss with --cert and --key".into(),
        ));
    }
    let users = boss.db().users().map_err(internal)?;
    let Some(owner) = users.iter().find(|u| u.id == user.id && !u.builtin) else {
        return Err(not_found(format!("user {}", user.id)));
    };
    if owner.certs.len() >= MOST_CERTIFICATES {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "{} has {MOST_CERTIFICATES} certificates already: revoke one first",
                owner.name
            ),
        ));
    }
    let label = next_label(&owner.name, owner.certs.iter().map(|c| &c.label));
    let name = owner.name.clone();
    let made = tokio::task::spawn_blocking({
        let label = label.clone();
        move || tls::client_certificate(&name, &label)
    });
    let made = made.await.map_err(internal)?.map_err(internal)?;
    boss.db()
        .add_certificate(user.id, &label, &made.fingerprint, &made.expires)
        .map_err(internal)?;
    Ok(Json(NewCertificate {
        label,
        p12: openssl::base64::encode_block(&made.p12),
        password: made.password,
    }))
}

#[derive(Deserialize)]
pub(super) struct CredentialId {
    pub(super) id: i64,
}

/// Revokes a client certificate or a passkey.
pub(super) async fn revoke_credential(
    State(boss): State<Shared>,
    Json(credential): Json<CredentialId>,
) -> Result<StatusCode, Failure> {
    check_lockout(&boss, |users| {
        for u in users.iter_mut() {
            if u.certs.iter().any(|c| c.id == credential.id) {
                u.certificates = u.certificates.saturating_sub(1);
                u.certs.retain(|c| c.id != credential.id);
            }
            if u.keys.iter().any(|k| k.id == credential.id) {
                u.passkeys = u.passkeys.saturating_sub(1);
                u.keys.retain(|k| k.id != credential.id);
            }
        }
    })?;
    boss.db()
        .revoke_credential(credential.id)
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// A user's passkeys, as webauthn-rs keeps them.
pub(super) fn user_passkeys(
    boss: &Boss,
    user: i64,
) -> Result<Vec<webauthn_rs::prelude::Passkey>, Failure> {
    let records = boss.db().user_passkeys(user).map_err(internal)?;
    records
        .iter()
        .map(|r| serde_json::from_str(r).map_err(internal))
        .collect()
}

#[derive(Deserialize)]
pub(super) struct PasskeyFor {
    // None: the user logged in.
    pub(super) id: Option<i64>,
}

/// Starts adding a passkey: the options for the browser's navigator.credentials.create(), in
/// WebAuthn's JSON form, and the ceremony's id. Users add passkeys for themselves; an admin may add
/// one for anyone, with an authenticator at hand (a security key to give them, say).
pub(super) async fn new_passkey_options(
    State(boss): State<Shared>,
    ConnectInfo(peer): ConnectInfo<tls::Peer>,
    headers: HeaderMap,
    Json(body): Json<PasskeyFor>,
) -> Result<Json<serde_json::Value>, Failure> {
    let (me, mine) = who(&boss, &headers, Some(&peer))?;
    let user = match (body.id, mine) {
        (Some(id), Some(mine)) if id == mine => id,
        (Some(id), _) if me.role == Role::Admin => id,
        (None, Some(mine)) => mine,
        _ if me.logged_in => {
            return Err((
                StatusCode::FORBIDDEN,
                format!("{} may add passkeys only for themselves", me.name),
            ));
        }
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                "log in first: a passkey is added for the user logged in".into(),
            ));
        }
    };
    let name = boss.db().user_name(user).map_err(internal)?;
    let name = name.ok_or_else(|| not_found(format!("user {user}")))?;
    let existing = user_passkeys(&boss, user)?;
    if existing.len() >= MOST_PASSKEYS {
        return Err((
            StatusCode::CONFLICT,
            format!("{name} has {MOST_PASSKEYS} passkeys already: revoke one first"),
        ));
    }
    // An authenticator that already holds one of the user's passkeys says so instead of making
    // another.
    let exclude = existing.iter().map(|k| k.cred_id().clone()).collect();
    let webauthn = boss.webauthn(&headers)?;
    let (options, state) = webauthn
        .start_passkey_registration(passkey::handle(user), &name, &name, Some(exclude))
        .map_err(internal)?;
    let mut options = serde_json::to_value(options.public_key).map_err(internal)?;
    // Discoverable, so logging in needs no name: the authenticator offers the boss's passkeys.
    options["authenticatorSelection"]["residentKey"] = "required".into();
    options["authenticatorSelection"]["requireResidentKey"] = true.into();
    let ceremony = boss.begin_ceremony(Ceremony::Register { user, state })?;
    Ok(Json(serde_json::json!({
        "ceremony": ceremony,
        "publicKey": options,
    })))
}

/// What the browser sends back to finish a ceremony: its id, and the credential in its JSON form
/// (PublicKeyCredential.toJSON()).
#[derive(Deserialize)]
pub(super) struct Finish<T> {
    pub(super) ceremony: String,
    pub(super) credential: T,
}

#[derive(Serialize)]
pub(super) struct PasskeyAdded {
    pub(super) label: String,
}

/// Finishes adding a passkey, once webauthn-rs finds that what the authenticator made checks out.
pub(super) async fn new_passkey(
    State(boss): State<Shared>,
    headers: HeaderMap,
    Json(finish): Json<Finish<RegisterPublicKeyCredential>>,
) -> Result<Json<PasskeyAdded>, Failure> {
    let bad = |why: String| (StatusCode::BAD_REQUEST, why);
    let Ceremony::Register { user, state } = boss.end_ceremony(&finish.ceremony)? else {
        return Err(bad("that ceremony was for logging in".into()));
    };
    let webauthn = boss.webauthn(&headers)?;
    let made = webauthn
        .finish_passkey_registration(&finish.credential, &state)
        .map_err(|e| bad(format!("the passkey does not check out: {e}")))?;
    let credential = passkey::credential_key(made.cred_id());
    let record = serde_json::to_value(&made).map_err(internal)?;
    let db = boss.db();
    let Some(name) = db.user_name(user).map_err(internal)? else {
        return Err(not_found(format!("user {user}")));
    };
    let keys = db.passkeys(user).map_err(internal)?;
    if keys.len() >= MOST_PASSKEYS {
        return Err((
            StatusCode::CONFLICT,
            format!("{name} has {MOST_PASSKEYS} passkeys already: revoke one first"),
        ));
    }
    if db.passkey(&credential).map_err(internal)?.is_some() {
        return Err((StatusCode::CONFLICT, "that passkey is added already".into()));
    }
    let label = next_label(&name, keys.iter().map(|k| &k.label));
    db.add_passkey(user, &label, &credential, &record)
        .map_err(internal)?;
    Ok(Json(PasskeyAdded { label }))
}

/// Starts logging in with a passkey: the options for navigator.credentials.get(), and the
/// ceremony's id. No name: the authenticator offers whichever of the boss's passkeys it holds.
pub(super) async fn passkey_login_options(
    State(boss): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Failure> {
    let webauthn = boss.webauthn(&headers)?;
    let (options, state) = webauthn
        .start_discoverable_authentication()
        .map_err(internal)?;
    let ceremony = boss.begin_ceremony(Ceremony::Login(state))?;
    Ok(Json(serde_json::json!({
        "ceremony": ceremony,
        "publicKey": options.public_key,
    })))
}

/// Logs in with a passkey, once webauthn-rs finds its signature over the ceremony's challenge
/// checks out (and its signature counter has not gone back, which would mean a copied key).
pub(super) async fn passkey_login(
    State(boss): State<Shared>,
    headers: HeaderMap,
    Json(finish): Json<Finish<PublicKeyCredential>>,
) -> Result<Response, Failure> {
    let refused = |why: String| (StatusCode::UNAUTHORIZED, why);
    let Ceremony::Login(state) = boss.end_ceremony(&finish.ceremony)? else {
        return Err(refused("that ceremony was for adding a passkey".into()));
    };
    let webauthn = boss.webauthn(&headers)?;
    let (handle, id) = webauthn
        .identify_discoverable_authentication(&finish.credential)
        .map_err(|e| refused(format!("the passkey says nothing of whose it is: {e}")))?;
    let credential = passkey::credential_key(id);
    let found = boss.db().passkey(&credential).map_err(internal)?;
    let found = found.filter(|(_, user, _)| passkey::user(handle) == *user);
    let Some((row, user, record)) = found else {
        return Err(refused(
            "that passkey is not, or no longer, anyone's".into(),
        ));
    };
    let mut key: webauthn_rs::prelude::Passkey = serde_json::from_str(&record).map_err(internal)?;
    let result = webauthn
        .finish_discoverable_authentication_v2(&finish.credential, state, &key)
        .map_err(|e| refused(format!("the passkey does not check out: {e}")))?;
    if key.update_credential(&result) == Some(true) {
        let record = serde_json::to_value(&key).map_err(internal)?;
        boss.db().set_passkey(row, &record).map_err(internal)?;
    }
    start_session(&boss, user)
}
