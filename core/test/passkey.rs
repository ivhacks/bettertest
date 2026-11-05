// Tests of passkey.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::*;

#[test]
fn user_ids_round_trip_through_handles() {
    for id in [1, 2, i64::MAX] {
        assert_eq!(user(handle(id)), id);
    }
}

#[test]
fn webauthn_is_for_the_host_the_browser_reached() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "localhost:7357".parse().unwrap());
    assert!(webauthn(&headers, false).is_ok());
    assert!(webauthn(&HeaderMap::new(), true).is_err());
}

#[test]
fn credential_keys_are_hex() {
    assert_eq!(credential_key(&[0x0b, 0xff, 0xfe]), "0bfffe");
}
