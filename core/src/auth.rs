// Who may do what. Every request acts as a user: the one whose session cookie it carries, or else
// the logged-out user (row 0), whose role starts as admin, so a boss nobody locked down works as it
// always did. Locking down means adding an admin who can log in, then lowering the logged-out role.

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use bettertest_shared::{Role, User};
use openssl::{rand::rand_bytes, sha::sha256};

pub const COOKIE: &str = "bettertest_session";
/// A session lasts this long since it was last used.
pub const SESSION_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// The role a request needs, from its method and path. None: anyone, even with role none, since
/// logging in has to work, and the pages and their assets carry no data (the API calls they make
/// are checked on their own).
pub fn needed(method: &str, path: &str) -> Option<Role> {
    let api = path.starts_with("/api/") || path.starts_with("/artifact/");
    // Adding a passkey checks who may on its own: users add their own, admins anyone's.
    let open = [
        "/api/login",
        "/api/logout",
        "/api/me",
        // A browser that cannot start the ui cannot have logged in.
        "/api/startup-error",
        "/api/passkey-login-options",
        "/api/passkey-login",
        "/api/new-passkey-options",
        "/api/new-passkey",
    ];
    if !api || open.contains(&path) {
        return None;
    }
    let admin = [
        "/api/users",
        "/api/add-user",
        "/api/set-role",
        "/api/set-password",
        "/api/remove-user",
        "/api/new-token",
        "/api/revoke-tokens",
        "/api/new-certificate",
        "/api/revoke-credential",
    ];
    if admin.contains(&path) || (path == "/api/settings" && method != "GET") {
        return Some(Role::Admin);
    }
    Some(if matches!(method, "GET" | "HEAD") {
        Role::Read
    } else {
        Role::Write
    })
}

/// Whether `users`, as they would be after a change, leave nobody able to administer the boss: the
/// logged-out user below admin, and no admin who can log in. The boss refuses such a change, since
/// undoing it would take editing the db by hand.
pub fn locks_out(users: &[User]) -> bool {
    let open = users.iter().any(|u| u.builtin && u.role == Role::Admin);
    let can_log_in = |u: &User| u.passwords + u.certificates + u.passkeys > 0;
    !open
        && !users
            .iter()
            .any(|u| !u.builtin && u.role == Role::Admin && can_log_in(u))
}

/// A password's Argon2id hash, as a PHC string (`$argon2id$v=19$m=19456,t=2,p=1$salt$hash`): the
/// argon2 crate's defaults, OWASP's recommended minimum, with a random salt.
pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

/// Whether `password` is the one `stored` is the hash of. The argon2 crate reads the cost from the
/// stored hash and compares in constant time.
pub fn check_password(password: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(hash) => Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok(),
        Err(_) => false,
    }
}

/// A new session token for the cookie, and the hash of it the db keeps: a stolen db then holds no
/// usable session.
pub fn new_token() -> Result<(String, String), String> {
    let mut token = [0u8; 32];
    rand_bytes(&mut token).map_err(|e| e.to_string())?;
    let token = hex(&token);
    let hash = token_hash(&token);
    Ok((token, hash))
}

pub fn token_hash(token: &str) -> String {
    // One round of sha256 is enough for 256 random bits; passwords are what need Argon2.
    hex(&sha256(token.as_bytes()))
}

/// The token in an `Authorization: Bearer <token>` header, if there is one.
pub fn bearer(authorization: &str) -> Option<&str> {
    authorization.strip_prefix("Bearer ").map(str::trim)
}

/// The session token in a Cookie header, if there is one.
pub fn session_token(cookies: &str) -> Option<&str> {
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE).then_some(value)
    })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "../test/auth.rs"]
mod tests;
