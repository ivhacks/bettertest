// Tests of auth.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;

fn user(name: &str, role: Role, builtin: bool, passwords: u32) -> User {
    User {
        id: 0,
        name: name.into(),
        role,
        builtin,
        passwords,
        certificates: 0,
        passkeys: 0,
        tokens: 0,
        certs: vec![],
        keys: vec![],
    }
}

#[test]
fn passwords_check_out_only_when_right() {
    let stored = hash_password("correct horse").unwrap();
    assert!(stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(check_password("correct horse", &stored));
    assert!(!check_password("correct hors", &stored));
    assert!(!check_password("correct horse", "plain text"));
    // Each hash has its own salt.
    assert_ne!(stored, hash_password("correct horse").unwrap());
}

#[test]
fn what_each_route_needs() {
    assert_eq!(needed("GET", "/pipeline/app"), None);
    assert_eq!(needed("GET", "/bettertest-frontend-1a2b.wasm"), None);
    assert_eq!(needed("POST", "/api/login"), None);
    assert_eq!(needed("GET", "/api/pipeline/app/events"), Some(Role::Read));
    assert_eq!(needed("GET", "/artifact/debian.qcow2"), Some(Role::Read));
    assert_eq!(needed("POST", "/api/start-run/app"), Some(Role::Write));
    assert_eq!(needed("PUT", "/artifact/debian.qcow2"), Some(Role::Write));
    assert_eq!(needed("GET", "/api/settings"), Some(Role::Read));
    assert_eq!(needed("PUT", "/api/settings"), Some(Role::Admin));
    assert_eq!(needed("GET", "/api/users"), Some(Role::Admin));
    assert_eq!(needed("POST", "/api/set-role"), Some(Role::Admin));
}

#[test]
fn nobody_can_lock_everyone_out() {
    let open = user("logged out", Role::Admin, true, 0);
    let closed = user("logged out", Role::None, true, 0);
    let iv = user("iv", Role::Admin, false, 1);
    let no_password = user("intern", Role::Admin, false, 0);
    let writer = user("ci", Role::Write, false, 1);
    // Open to everyone: nothing to lock out.
    assert!(!locks_out(&[open, writer.clone()]));
    // Closed, with an admin who can log in: fine.
    assert!(!locks_out(&[closed.clone(), iv]));
    // Closed, with only an admin who cannot log in, or none at all: refused.
    assert!(locks_out(&[closed.clone(), no_password]));
    assert!(locks_out(&[closed, writer]));
}

#[test]
fn the_session_cookie_among_others() {
    assert_eq!(
        session_token("a=1; bettertest_session=abc; b=2"),
        Some("abc")
    );
    assert_eq!(session_token("a=1"), None);
}
