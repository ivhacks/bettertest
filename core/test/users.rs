// Tests of boss/users.rs: who a request is, logins, tokens, lockout and passkey ceremonies, through
// the boss's own router and guard over HTTP, as a browser or curl meets them. A child of the boss's
// tests (see the #[path] in test/boss.rs), so it reaches what the boss keeps private.

use super::*;
use serde_json::{Value, json};

/// A boss with an empty db, served through its router and guard as `entry` serves it.
async fn served() -> (Shared, String) {
    let boss = boss(BTreeMap::new());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let service = router(boss.clone()).into_make_service_with_connect_info::<tls::Peer>();
    spawn(async move { axum::serve(tls::PlainListener(listener), service).await });
    (boss, url)
}

/// Someone calling the boss: a browser with its session cookie, curl with a token, or neither.
#[derive(Clone)]
struct Caller {
    url: String,
    http: reqwest::Client,
    // A whole Cookie or Authorization header.
    credential: Option<(header::HeaderName, String)>,
}

/// What the boss answered: its status, its Set-Cookie header if any, and its body.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    cookie: Option<String>,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

impl Caller {
    fn anyone(url: &str) -> Caller {
        let http = reqwest::Client::new();
        Caller {
            url: url.into(),
            http,
            credential: None,
        }
    }

    fn with_cookie(&self, cookie: &str) -> Caller {
        let credential = Some((header::COOKIE, cookie.to_string()));
        Caller {
            credential,
            ..self.clone()
        }
    }

    fn with_token(&self, token: &str) -> Caller {
        let credential = Some((header::AUTHORIZATION, format!("Bearer {token}")));
        Caller {
            credential,
            ..self.clone()
        }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Answer {
        let mut request = self.http.request(method, format!("{}{path}", self.url));
        // Passkeys are bound to a host name, and WebAuthn takes no IP address for one.
        request = request.header(header::HOST, "localhost:7357");
        if let Some((name, value)) = &self.credential {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let cookie = response.headers().get(header::SET_COOKIE);
        let cookie = cookie.map(|c| c.to_str().unwrap().to_string());
        let body = response.text().await.unwrap();
        Answer {
            status,
            cookie,
            body,
        }
    }

    async fn get(&self, path: &str) -> Answer {
        self.call(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Answer {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    /// Logs in as `name`, and is that user from then on.
    async fn log_in(&self, name: &str, password: &str) -> Caller {
        let answer = self
            .post("/api/login", json!({"name": name, "password": password}))
            .await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
        let cookie = answer.cookie.unwrap();
        let pair = cookie.split(';').next().unwrap();
        self.with_cookie(pair)
    }
}

/// The id of user `name`.
fn user_id(boss: &Boss, name: &str) -> i64 {
    let users = boss.db().users().unwrap();
    users.iter().find(|u| u.name == name).unwrap().id
}

async fn add_user(caller: &Caller, name: &str, role: &str) -> Answer {
    let user = json!({"name": name, "role": role, "password": "correct horse"});
    caller.post("/api/add-user", user).await
}

#[tokio::test]
async fn logging_in_starts_a_session_and_logging_out_ends_it() {
    let (_, url) = served().await;
    let anyone = Caller::anyone(&url);
    assert_eq!(
        add_user(&anyone, "iv", "admin").await.status,
        StatusCode::CREATED
    );

    // An unknown name gets the same answer as a wrong password: it says nothing of who exists.
    for (name, password) in [("iv", "correct hors"), ("nobody", "correct horse")] {
        let refused = anyone
            .post("/api/login", json!({"name": name, "password": password}))
            .await;
        assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
        assert_eq!(refused.body, "that name and password do not match");
        assert_eq!(refused.cookie, None);
    }

    let login = json!({"name": "iv", "password": "correct horse"});
    let answer = anyone.post("/api/login", login).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let cookie = answer.cookie.unwrap();
    let (pair, attributes) = cookie.split_once("; ").unwrap();
    let token = pair.strip_prefix("bettertest_session=").unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        attributes,
        "Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000"
    );
    let iv = anyone.with_cookie(pair);
    let me = json!({"name": "iv", "role": "admin", "logged_in": true});
    assert_eq!(iv.get("/api/me").await.json(), me);
    let logged_out = json!({"name": "logged out", "role": "admin", "logged_in": false});
    assert_eq!(anyone.get("/api/me").await.json(), logged_out);
    // A cookie the boss never handed out is nobody's.
    let forged = anyone.with_cookie(&format!("bettertest_session={}", "0".repeat(64)));
    assert_eq!(forged.get("/api/me").await.json(), logged_out);

    let out = iv.post("/api/logout", json!({})).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert_eq!(
        out.cookie.as_deref(),
        Some("bettertest_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    // The session is over on the boss too, not only gone from this browser.
    assert_eq!(iv.get("/api/me").await.json(), logged_out);
}

/// A session lasts 30 days since it was last used: one in use is pushed out, at most once a day, and
/// one past its time logs nobody in.
#[tokio::test]
async fn a_session_in_use_lasts_and_an_old_one_does_not() {
    let (boss, url) = served().await;
    let anyone = Caller::anyone(&url);
    add_user(&anyone, "iv", "admin").await;
    let iv = user_id(&boss, "iv");
    let day = 24 * 60 * 60 * 1000;
    let session = |expires: u64| {
        let (token, hash) = auth::new_token().unwrap();
        boss.db().add_session(&hash, iv, expires).unwrap();
        let caller = anyone.with_cookie(&format!("a=1; bettertest_session={token}"));
        (caller, hash)
    };
    let expires = |hash: &str| boss.db().session(hash, 0).unwrap().map(|s| s.3);
    let start = now();

    let (used, hash) = session(start + 2 * day);
    assert_eq!(used.get("/api/me").await.json()["name"], "iv");
    let pushed = expires(&hash).unwrap();
    assert!(pushed >= start + auth::SESSION_MS, "{pushed}");

    // Used again within the day: not written again.
    let (fresh, hash) = session(start + auth::SESSION_MS - 60 * 60 * 1000);
    assert_eq!(fresh.get("/api/me").await.json()["name"], "iv");
    assert_eq!(
        expires(&hash),
        Some(start + auth::SESSION_MS - 60 * 60 * 1000)
    );

    let (old, _) = session(start - 1);
    assert_eq!(old.get("/api/me").await.json()["logged_in"], false);
}

#[tokio::test]
async fn locking_down_takes_an_admin_who_can_log_in_and_then_roles_count() {
    let (boss, url) = served().await;
    let anyone = Caller::anyone(&url);
    let lower_logged_out = json!({"id": 0, "role": "none"});
    let refused = anyone.post("/api/set-role", lower_logged_out.clone()).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(
        refused.body,
        "that would leave nobody able to administer bettertest: add an admin who can log in \
         first, or keep logged out at admin"
    );

    add_user(&anyone, "iv", "admin").await;
    add_user(&anyone, "ci", "write").await;
    let done = anyone.post("/api/set-role", lower_logged_out).await;
    assert_eq!(done.status, StatusCode::NO_CONTENT);

    // Logged out may now do nothing but log in.
    let pipelines = anyone.get("/api/pipelines").await;
    assert_eq!(pipelines.status, StatusCode::UNAUTHORIZED);
    assert_eq!(pipelines.body, "log in to do this: it needs read access");
    let me = json!({"name": "logged out", "role": "none", "logged_in": false});
    assert_eq!(anyone.get("/api/me").await.json(), me);

    let ci = anyone.log_in("ci", "correct horse").await;
    assert_eq!(ci.get("/api/pipelines").await.status, StatusCode::OK);
    let users = ci.get("/api/users").await;
    assert_eq!(users.status, StatusCode::FORBIDDEN);
    assert_eq!(users.body, "ci has write access, and this needs admin");

    let iv = anyone.log_in("iv", "correct horse").await;
    let users = iv.get("/api/users").await.json();
    let names: Vec<&str> = (users.as_array().unwrap().iter())
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    // Logged out first, then by name.
    assert_eq!(names, ["logged out", "ci", "iv"]);
    // The only admin who can log in stays: neither removed nor lowered.
    let iv_id = user_id(&boss, "iv");
    let removed = iv.post("/api/remove-user", json!({"id": iv_id})).await;
    assert_eq!(removed.status, StatusCode::CONFLICT);
    let lowered = iv
        .post("/api/set-role", json!({"id": iv_id, "role": "write"}))
        .await;
    assert_eq!(lowered.status, StatusCode::CONFLICT);

    let ci_id = user_id(&boss, "ci");
    let read = iv
        .post("/api/set-role", json!({"id": ci_id, "role": "read"}))
        .await;
    assert_eq!(read.status, StatusCode::NO_CONTENT);
    let start = ci.post("/api/start-run/app", json!({})).await;
    assert_eq!(start.status, StatusCode::FORBIDDEN);
    assert_eq!(start.body, "ci has read access, and this needs write");

    // A new password is the one that logs in from then on.
    let set = json!({"id": ci_id, "password": "battery staple"});
    let changed = iv.post("/api/set-password", set).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT);
    let old = json!({"name": "ci", "password": "correct horse"});
    assert_eq!(
        anyone.post("/api/login", old).await.status,
        StatusCode::UNAUTHORIZED
    );
    anyone.log_in("ci", "battery staple").await;

    // A removed user's session logs nobody in.
    let gone = iv.post("/api/remove-user", json!({"id": ci_id})).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert_eq!(
        ci.get("/api/pipelines").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn tokens_act_as_their_user_until_revoked_and_a_task_only_reads() {
    let (boss, url) = served().await;
    let anyone = Caller::anyone(&url);
    add_user(&anyone, "iv", "admin").await;
    add_user(&anyone, "forge", "write").await;
    anyone
        .post("/api/set-role", json!({"id": 0, "role": "none"}))
        .await;
    let iv = anyone.log_in("iv", "correct horse").await;
    let forge_id = user_id(&boss, "forge");

    let made = iv
        .post("/api/new-token", json!({"id": forge_id}))
        .await
        .json();
    let token = made["token"].as_str().unwrap();
    let forge = anyone.with_token(token);
    let me = json!({"name": "forge", "role": "write", "logged_in": true});
    assert_eq!(forge.get("/api/me").await.json(), me);
    assert_eq!(forge.get("/api/pipelines").await.status, StatusCode::OK);
    let users = boss.db().users().unwrap();
    let counted = users.iter().find(|u| u.id == forge_id).unwrap().tokens;
    assert_eq!(counted, 1);

    // A token that is not one is refused outright, not taken for logged out.
    let wrong = anyone.with_token("not-a-token").get("/api/pipelines").await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body, "that token is not, or no longer, valid");

    let revoked = iv.post("/api/revoke-tokens", json!({"id": forge_id})).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let after = forge.get("/api/pipelines").await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
    assert_eq!(after.body, "that token is not, or no longer, valid");

    let none = iv.post("/api/new-token", json!({"id": 0})).await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
    assert_eq!(none.body, "logged out needs no token");

    // A running task's token reads what its run needs from a boss locked down to none, and
    // nothing more.
    let (task_token, hash) = auth::new_token().unwrap();
    lock(&boss.task_tokens).insert(hash, Uuid::new_v4());
    let task = anyone.with_token(&task_token);
    assert_eq!(task.get("/api/pipelines").await.status, StatusCode::OK);
    let start = task.post("/api/start-run/app", json!({})).await;
    assert_eq!(start.status, StatusCode::FORBIDDEN);
    assert_eq!(
        start.body,
        "a running task has read access, and this needs write"
    );
}

#[tokio::test]
async fn user_changes_that_make_no_sense_are_refused() {
    let (_, url) = served().await;
    let anyone = Caller::anyone(&url);
    add_user(&anyone, "iv", "admin").await;
    let refusals = [
        (
            "/api/add-user",
            json!({"name": " ", "role": "read", "password": "correct horse"}),
            StatusCode::BAD_REQUEST,
            "\"\" cannot be a user's name",
        ),
        (
            "/api/add-user",
            json!({"name": "logged out", "role": "read", "password": "correct horse"}),
            StatusCode::BAD_REQUEST,
            "\"logged out\" cannot be a user's name",
        ),
        (
            "/api/add-user",
            json!({"name": "ci", "role": "read", "password": "short"}),
            StatusCode::BAD_REQUEST,
            "a password has at least 8 characters",
        ),
        (
            "/api/add-user",
            json!({"name": "iv", "role": "read", "password": "correct horse"}),
            StatusCode::CONFLICT,
            "there is a user named iv already",
        ),
        (
            "/api/set-password",
            json!({"id": 0, "password": "correct horse"}),
            StatusCode::BAD_REQUEST,
            "logged out has no password",
        ),
        (
            "/api/remove-user",
            json!({"id": 0}),
            StatusCode::BAD_REQUEST,
            "logged out is always there; lower its role instead",
        ),
        (
            "/api/new-certificate",
            json!({"id": 1}),
            StatusCode::CONFLICT,
            "certificate login needs HTTPS: start the boss with --cert and --key",
        ),
    ];
    for (path, body, status, said) in refusals {
        let answer = anyone.post(path, body).await;
        assert_eq!(
            (answer.status, answer.body.as_str()),
            (status, said),
            "{path}"
        );
    }
    let https = anyone.get("/api/https").await;
    assert_eq!(
        (https.status, https.body.as_str()),
        (StatusCode::OK, "null")
    );
}

/// What navigator.credentials.get() hands back, in its JSON form, from an authenticator that
/// signed nothing this boss can check.
fn signed_nothing() -> Value {
    json!({
        "id": "AAAA",
        "rawId": "AAAA",
        "response": {
            "authenticatorData": "AAAA",
            "clientDataJSON": "AAAA",
            "signature": "AAAA",
            "userHandle": null,
        },
        "extensions": {},
        "type": "public-key",
    })
}

#[tokio::test]
async fn passkeys_are_added_by_their_user_or_an_admin_and_a_ceremony_works_once() {
    let (boss, url) = served().await;
    let anyone = Caller::anyone(&url);
    add_user(&anyone, "iv", "admin").await;
    add_user(&anyone, "ci", "write").await;
    let iv_id = user_id(&boss, "iv");

    // Logged out may administer this boss, so it may add one for anyone, but none for "itself".
    let mine = anyone
        .post("/api/new-passkey-options", json!({"id": null}))
        .await;
    assert_eq!(mine.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        mine.body,
        "log in first: a passkey is added for the user logged in"
    );
    anyone
        .post("/api/set-role", json!({"id": 0, "role": "read"}))
        .await;
    let ci = anyone.log_in("ci", "correct horse").await;
    let theirs = ci
        .post("/api/new-passkey-options", json!({"id": iv_id}))
        .await;
    assert_eq!(theirs.status, StatusCode::FORBIDDEN);
    assert_eq!(theirs.body, "ci may add passkeys only for themselves");

    let options = ci
        .post("/api/new-passkey-options", json!({"id": null}))
        .await;
    assert_eq!(options.status, StatusCode::OK, "{}", options.body);
    let options = options.json();
    let public_key = &options["publicKey"];
    assert_eq!(public_key["rp"]["id"], "localhost");
    assert_eq!(public_key["user"]["name"], "ci");
    // Discoverable, so that logging in needs no name.
    assert_eq!(
        public_key["authenticatorSelection"]["residentKey"],
        "required"
    );
    assert_eq!(
        public_key["authenticatorSelection"]["requireResidentKey"],
        true
    );
    let ceremony = options["ceremony"].as_str().unwrap();

    // A ceremony for adding a passkey does not log anyone in, and is over once tried.
    let finish = json!({"ceremony": ceremony, "credential": signed_nothing()});
    let login = anyone.post("/api/passkey-login", finish.clone()).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login.body, "that ceremony was for adding a passkey");
    let again = anyone.post("/api/passkey-login", finish).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        again.body,
        "that passkey ceremony is unknown, finished or expired: try again"
    );

    // A login ceremony checks the signature, and finds none here.
    let options = anyone
        .post("/api/passkey-login-options", json!({}))
        .await
        .json();
    assert_eq!(options["publicKey"]["rpId"], "localhost");
    let ceremony = options["ceremony"].as_str().unwrap();
    let finish = json!({"ceremony": ceremony, "credential": signed_nothing()});
    let login = anyone.post("/api/passkey-login", finish).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    assert!(
        login
            .body
            .starts_with("the passkey says nothing of whose it is: "),
        "{}",
        login.body
    );
    assert_eq!(login.cookie, None);
}

#[test]
fn passkey_ceremonies_are_bounded_and_expire() {
    let boss = boss(BTreeMap::new());
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "localhost:7357".parse().unwrap());
    let webauthn = boss.webauthn(&headers).unwrap();
    let login = || {
        let (_, state) = webauthn.start_discoverable_authentication().unwrap();
        Ceremony::Login(state)
    };
    let first = boss.begin_ceremony(login()).unwrap();
    for _ in 1..MOST_CEREMONIES {
        boss.begin_ceremony(login()).unwrap();
    }
    let full = boss.begin_ceremony(login()).map(|_| ()).unwrap_err();
    assert_eq!(
        full,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many passkey ceremonies under way: try again in a few minutes".into()
        )
    );
    // One that ran out of time makes room, and cannot be finished.
    lock(&boss.ceremonies).get_mut(&first).unwrap().expires = 0;
    let made = boss.begin_ceremony(login()).unwrap();
    assert!(boss.end_ceremony(&first).is_err());
    assert!(matches!(boss.end_ceremony(&made), Ok(Ceremony::Login(_))));
    assert!(boss.end_ceremony(&made).is_err());
}
