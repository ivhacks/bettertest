// A git repository whose `.bettertest/*.py` files are pipelines. The boss keeps a bare mirror of it
// under /var/tmp/bettertest/repos, fetched when it starts, when a run starts and when a push
// arrives, and reads pipedefs and source trees from there. Workers never need git or the repo's
// credentials: they get a commit's files from the boss as a tarball.

use crate::{client, workdir, worker::cache_key};
use bettertest_shared::Commit;
use reqwest::{Client, Url};
use std::{path::PathBuf, process::Stdio};
use tokio::process::Command;

const PIPEDEFS: &str = ".bettertest/";

pub struct Repo {
    // As given, credentials included: git fetches with it, and the forge's API takes them too.
    url: Url,
    // The url without credentials, for logs.
    pub public: String,
    // owner/name, as the forge calls it.
    pub full_name: String,
    dir: PathBuf,
}

impl Repo {
    pub fn new(url: &str) -> Result<Repo, String> {
        let url = Url::parse(url).map_err(|e| format!("{url} is not a url: {e}"))?;
        let mut public = url.clone();
        // Only fails for urls that cannot have credentials, which then have none to remove.
        let _ = public.set_username("");
        let _ = public.set_password(None);
        let full_name = url
            .path()
            .trim_matches('/')
            .trim_end_matches(".git")
            .to_string();
        if full_name.split('/').count() != 2 {
            return Err(format!(
                "{public} is not a repo url like http://host/owner/name.git"
            ));
        }
        let dir = workdir::dir("repos")
            .map_err(|e| e.to_string())?
            .join(cache_key(public.as_str()));
        Ok(Repo {
            url,
            public: public.to_string(),
            full_name,
            dir,
        })
    }

    /// Runs git in the mirror. Ok is its stdout; Err its complaint, without the credentials.
    async fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            // Never ask anyone for a password, never remember one.
            .args(["-c", "credential.helper=", "-c", "core.askPass=true"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| format!("could not run git: {e}"))?;
        if !output.status.success() {
            let complaint = String::from_utf8_lossy(&output.stderr);
            return Err(
                self.hide_credentials(&format!("git {} failed: {complaint}", args.join(" ")))
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs git against the forge. The url git sees has no credentials, so they never show in the
    /// process list: git finds them through its `store` helper, in a file only this user can read,
    /// written for this one call and removed after it.
    async fn remote(&self, args: &[&str]) -> Result<String, String> {
        let password = self.url.password().unwrap_or_default();
        if self.url.username().is_empty() && password.is_empty() {
            return self.git(args).await;
        }
        let mut forge = self.url.clone();
        forge.set_path("");
        let file = Secret::write(&format!("{}\n", forge.as_str().trim_end_matches('/')))
            .map_err(|e| format!("could not hand git the credentials: {e}"))?;
        let helper = format!("credential.helper=store --file={}", file.path.display());
        let mut with_helper = vec!["-c", &helper];
        with_helper.extend_from_slice(args);
        self.git(&with_helper).await
    }

    fn hide_credentials(&self, text: &str) -> String {
        match self.url.password() {
            Some(password) if !password.is_empty() => text.replace(password, "***"),
            _ => text.to_string(),
        }
    }

    /// Brings the mirror up to date with every branch, and its HEAD with the default branch.
    pub async fn fetch(&self) -> Result<(), String> {
        if !self.dir.join("HEAD").exists() {
            workdir::shared_dir(&self.dir).map_err(|e| e.to_string())?;
            // 0666 files and 0777 directories, so intern 2 can fetch into intern 1's mirror.
            self.git(&["init", "--quiet", "--bare", "--shared=0666", "."])
                .await?;
        }
        // The url goes on the command line rather than into the mirror's config, and without its
        // credentials: those reach git through `remote`.
        let url = self.public.as_str();
        let refs = self.remote(&["ls-remote", "--symref", url, "HEAD"]).await?;
        let default = refs
            .lines()
            .find_map(|line| line.strip_prefix("ref: ")?.strip_suffix("\tHEAD"))
            .ok_or_else(|| format!("{} has no default branch", self.public))?
            .to_string();
        self.remote(&[
            "fetch",
            "--quiet",
            // git writes FETCH_HEAD with the umask's mode, not the mirror's 0666.
            "--no-write-fetch-head",
            "--prune",
            "--force",
            url,
            "+refs/heads/*:refs/heads/*",
        ])
        .await?;
        self.git(&["symbolic-ref", "HEAD", &default]).await?;
        Ok(())
    }

    /// The default branch's newest commit, as of the last fetch.
    pub async fn head(&self) -> Result<Commit, String> {
        let sha = self.git(&["rev-parse", "HEAD"]).await?.trim().to_string();
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"]).await?;
        Ok(Commit {
            sha,
            branch: branch.trim().to_string(),
        })
    }

    /// The pipedefs in commit `sha` (or `HEAD`), as (pipeline name, path in the repo).
    pub async fn pipedefs(&self, sha: &str) -> Result<Vec<(String, String)>, String> {
        let rev = checked(sha)?;
        let listing = self
            .git(&["ls-tree", "--name-only", rev, "--", PIPEDEFS])
            .await?;
        Ok(listing
            .lines()
            .filter_map(|path| {
                let name = path.strip_prefix(PIPEDEFS)?.strip_suffix(".py")?;
                Some((name.to_string(), path.to_string()))
            })
            .collect())
    }

    /// The file at `path` in commit `sha` (or `HEAD`).
    pub async fn show(&self, sha: &str, path: &str) -> Result<String, String> {
        let rev = checked(sha)?;
        self.git(&["show", &format!("{rev}:{path}")]).await
    }

    /// `git archive` of commit `sha` as a tar stream on stdout.
    pub fn archive(&self, sha: &str) -> Result<Command, String> {
        let rev = checked(sha)?;
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.dir)
            .args(["archive", "--format=tar", rev])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        Ok(command)
    }

    /// Tells the forge how pipeline `context` did on commit `sha`: `state` is pending, success,
    /// failure or warning, `description` says it in words, and `link` is where to see it.
    /// Forgejo and Gitea take `/api/v1` on the repo's host. GitHub has no such path (it answers
    /// 410); a github.com repo is reported to api.github.com instead.
    pub async fn report(
        &self,
        http: &Client,
        sha: &str,
        context: &str,
        state: &str,
        description: &str,
        link: Option<&str>,
    ) -> Result<(), String> {
        let call = status_call(
            &self.url,
            &self.full_name,
            sha,
            context,
            state,
            description,
            link,
        )?;
        // The token stays out of the url, which is what a failure logs.
        let token = credential(&self.url);
        let response = status_request(
            http,
            &call,
            token.as_deref(),
            self.url.username(),
            self.url.password(),
        )
        .send()
        .await
        .map_err(|e| format!("could not reach {}: {}", call.url, client::why(&e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("{} said {status}: {text}", call.url));
        }
        Ok(())
    }
}

/// One commit-status post: where, and the JSON body. `github` selects GitHub's auth and response
/// shape. The url has no credentials.
#[derive(Debug)]
struct StatusCall {
    url: Url,
    body: serde_json::Value,
    github: bool,
}

/// The password of `url`, or its username when the token is the whole credential
/// (`https://<token>@host/owner/name.git`).
fn credential(url: &Url) -> Option<String> {
    match url.password() {
        Some(password) if !password.is_empty() => Some(password.to_string()),
        _ => {
            let user = url.username();
            if user.is_empty() {
                None
            } else {
                Some(user.to_string())
            }
        }
    }
}

fn is_github(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
    })
}

/// GitHub rejects a status post that has no User-Agent with 403 ("administrative rules"), before
/// it even checks the token. The header is set on the request so a client whose default agent is
/// empty still satisfies that rule. The token is a bearer header, never part of `call.url`.
fn status_request(
    http: &Client,
    call: &StatusCall,
    token: Option<&str>,
    user: &str,
    password: Option<&str>,
) -> reqwest::RequestBuilder {
    let request = http.post(call.url.clone()).json(&call.body);
    if call.github {
        let request = request
            .header("user-agent", "bettertest")
            .header("accept", "application/vnd.github+json");
        match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    } else {
        request.basic_auth(user, password)
    }
}

/// Forgejo's and Gitea's status API on the repo's own host, or GitHub's on api.github.com.
/// GitHub has no `warning` state and keeps at most 140 characters of description.
fn status_call(
    repo: &Url,
    full_name: &str,
    sha: &str,
    context: &str,
    state: &str,
    description: &str,
    link: Option<&str>,
) -> Result<StatusCall, String> {
    let sha = checked(sha)?;
    let github = is_github(repo);
    let mut url = if github {
        Url::parse("https://api.github.com/").map_err(|e| e.to_string())?
    } else {
        let mut url = repo.clone();
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url
    };
    let path = if github {
        format!("/repos/{full_name}/statuses/{sha}")
    } else {
        format!("/api/v1/repos/{full_name}/statuses/{sha}")
    };
    url.set_path(&path);
    // A cancelled run is `warning` on Forgejo and Gitea. GitHub would reject that state.
    let state = if github && state == "warning" {
        "failure"
    } else {
        state
    };
    let description = if github {
        description.chars().take(140).collect::<String>()
    } else {
        description.to_string()
    };
    let mut body = serde_json::json!({
        "state": state,
        "context": format!("bettertest/{context}"),
        "description": description,
    });
    if let Some(link) = link {
        body["target_url"] = serde_json::Value::String(link.to_string());
    } else if !github {
        body["target_url"] = serde_json::Value::Null;
    }
    Ok(StatusCall { url, body, github })
}

/// A file only this user can read, removed when dropped. Not 0666 like everything else in the work
/// directory: what is in it is a secret, and it lives only as long as one git command.
struct Secret {
    path: PathBuf,
}

impl Secret {
    fn write(text: &str) -> std::io::Result<Secret> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let path =
            workdir::dir("repos")?.join(format!(".credentials-{}", uuid::Uuid::new_v4().simple()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let secret = Secret { path };
        file.write_all(text.as_bytes())?;
        Ok(secret)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A commit id, or HEAD. Anything else could be read by git as an option.
fn checked(sha: &str) -> Result<&str, String> {
    let hex = !sha.is_empty() && sha.len() <= 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
    if hex || sha == "HEAD" {
        Ok(sha)
    } else {
        Err(format!("{sha:?} is not a commit id"))
    }
}

#[cfg(test)]
#[path = "../test/repo.rs"]
mod tests;
