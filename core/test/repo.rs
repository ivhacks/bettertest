// Tests of repo.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::{credential, status_call};
use reqwest::Url;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn github_status_uses_api_github_com() {
    let repo = url("https://ci:secret@github.com/ivhacks/bettertest.git");
    let call = status_call(
        &repo,
        "ivhacks/bettertest",
        "e0346054145650540e333eba6e18d3a853a53823",
        "bettertest",
        "pending",
        "running",
        Some("https://boss.example/pipeline/bettertest"),
    )
    .unwrap();
    assert!(call.github);
    assert_eq!(
        call.url.as_str(),
        "https://api.github.com/repos/ivhacks/bettertest/statuses/e0346054145650540e333eba6e18d3a853a53823"
    );
    assert!(!call.url.as_str().contains("secret"));
    assert_eq!(call.body["state"], "pending");
    assert_eq!(call.body["context"], "bettertest/bettertest");
    assert_eq!(call.body["description"], "running");
    assert_eq!(
        call.body["target_url"],
        "https://boss.example/pipeline/bettertest"
    );

    let www = status_call(
        &url("https://www.github.com/ivhacks/bettertest.git"),
        "ivhacks/bettertest",
        "abc",
        "bettertest",
        "success",
        "passed",
        None,
    )
    .unwrap();
    assert!(www.github);
    assert_eq!(
        www.url.as_str(),
        "https://api.github.com/repos/ivhacks/bettertest/statuses/abc"
    );
    assert!(www.body.get("target_url").is_none());
}

#[test]
fn github_warning_becomes_failure_and_description_is_capped() {
    let long = "x".repeat(180);
    let call = status_call(
        &url("https://github.com/ivhacks/bettertest.git"),
        "ivhacks/bettertest",
        "abc",
        "bettertest",
        "warning",
        &long,
        None,
    )
    .unwrap();
    assert_eq!(call.body["state"], "failure");
    assert_eq!(
        call.body["description"].as_str().unwrap().chars().count(),
        140
    );
    assert!(
        call.body["description"]
            .as_str()
            .unwrap()
            .chars()
            .all(|c| c == 'x')
    );
}

#[test]
fn gitea_status_stays_on_the_repo_host() {
    let call = status_call(
        &url("http://ci:secret@forge.example/team/app.git"),
        "team/app",
        "abc",
        "app",
        "warning",
        "cancelled: main replaced it",
        None,
    )
    .unwrap();
    assert!(!call.github);
    assert_eq!(
        call.url.as_str(),
        "http://forge.example/api/v1/repos/team/app/statuses/abc"
    );
    assert!(!call.url.as_str().contains("secret"));
    assert_eq!(call.body["state"], "warning");
    assert_eq!(call.body["description"], "cancelled: main replaced it");
    assert_eq!(call.body["context"], "bettertest/app");
    assert_eq!(call.body["target_url"], serde_json::Value::Null);
}

#[test]
fn credential_is_the_password_or_else_the_username() {
    assert_eq!(
        credential(&url("https://ci:secret@github.com/a/b.git")).as_deref(),
        Some("secret")
    );
    assert_eq!(
        credential(&url("https://ghp_token@github.com/a/b.git")).as_deref(),
        Some("ghp_token")
    );
    assert_eq!(credential(&url("https://github.com/a/b.git")), None);
}

#[test]
fn status_refuses_a_sha_git_could_read_as_an_option() {
    let err = status_call(
        &url("https://github.com/ivhacks/bettertest.git"),
        "ivhacks/bettertest",
        "--upload-pack",
        "bettertest",
        "pending",
        "running",
        None,
    )
    .unwrap_err();
    assert!(err.contains("not a commit id"), "{err}");
}
