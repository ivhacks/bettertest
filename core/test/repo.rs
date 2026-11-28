// Tests of repo.rs, a child module of it (see the #[path] at its end), so they
// reach what it keeps private.

use super::{credential, status_call, status_request};
use reqwest::Url;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn github_status_uses_api_github_com() {
    let repo = url("https://ci:secret@github.com/ivhacks/bettertest.git");
    let call = status_call(
        &repo,
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
fn github_status_names_itself_and_keeps_the_token_out_of_the_url() {
    let http = reqwest::Client::new();
    let repo = url("https://ci:secret@github.com/ivhacks/bettertest.git");
    let call = status_call(&repo, "abc", "bettertest", "pending", "running", None).unwrap();
    let sent = status_request(&http, &call, &repo).build().unwrap();
    assert_eq!(sent.headers().get("user-agent").unwrap(), "bettertest");
    assert_eq!(
        sent.headers().get("accept").unwrap(),
        "application/vnd.github+json"
    );
    let authorization = sent
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(authorization.starts_with("Bearer "), "{authorization}");
    assert!(authorization.contains("secret"), "{authorization}");
    assert!(!sent.url().as_str().contains("secret"));
    assert!(!sent.url().as_str().contains("ci:"));

    let public = url("https://github.com/ivhacks/bettertest.git");
    let anonymous = status_request(&http, &call, &public).build().unwrap();
    assert_eq!(anonymous.headers().get("user-agent").unwrap(), "bettertest");
    assert!(anonymous.headers().get("authorization").is_none());
}

#[test]
fn gitea_status_sends_the_repo_credentials_as_basic_auth() {
    let http = reqwest::Client::new();
    let repo = url("http://ci:secret@forge.example/team/app.git");
    let call = status_call(&repo, "abc", "app", "pending", "running", None).unwrap();
    let sent = status_request(&http, &call, &repo).build().unwrap();
    let authorization = sent.headers().get("authorization").unwrap();
    // base64 of ci:secret
    assert_eq!(authorization, "Basic Y2k6c2VjcmV0");
    assert!(sent.headers().get("user-agent").is_none());
    assert!(!sent.url().as_str().contains("secret"));
}

#[test]
fn status_refuses_a_sha_git_could_read_as_an_option() {
    let err = status_call(
        &url("https://github.com/ivhacks/bettertest.git"),
        "--upload-pack",
        "bettertest",
        "pending",
        "running",
        None,
    )
    .unwrap_err();
    assert!(err.contains("not a commit id"), "{err}");
}
