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
    /// Forgejo's and Gitea's commit status API.
    pub async fn report(
        &self,
        http: &Client,
        sha: &str,
        context: &str,
        state: &str,
        description: &str,
        link: Option<&str>,
    ) -> Result<(), String> {
        let mut api = self.url.clone();
        let _ = api.set_username("");
        let _ = api.set_password(None);
        api.set_path(&format!(
            "/api/v1/repos/{}/statuses/{}",
            self.full_name,
            checked(sha)?
        ));
        let body = serde_json::json!({
            "state": state,
            "context": format!("bettertest/{context}"),
            "description": description,
            "target_url": link,
        });
        let user = self.url.username();
        let response = http
            .post(api.clone())
            .basic_auth(user, self.url.password())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("could not reach {api}: {}", client::why(&e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("{api} said {status}: {text}"));
        }
        Ok(())
    }
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
