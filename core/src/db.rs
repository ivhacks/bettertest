use crate::workdir;
use bettertest_shared::*;
use rusqlite::{Connection, OptionalExtension, Row, types::Type};
use std::{collections::BTreeMap, error::Error};
use uuid::Uuid;

/// The schema of a new db. Until bettertest is released, a db that has another one is deleted, not
/// upgraded.
const SCHEMA: &str = include_str!("../schema.sql");

pub struct Db {
    conn: Connection,
}

pub fn open() -> Result<Db, Box<dyn Error>> {
    let path = workdir::root()?.join("bettertest.db");
    // sqlite gives the -wal and -shm files the same permissions as the db file, so making this
    // one 0666 lets every user write all three.
    workdir::shared_file(&path)?;
    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // In WAL mode NORMAL never corrupts the db. A power cut can only lose the newest commits.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(prepare(conn)?)
}

/// Turns on foreign keys, and gives a new db the schema.
pub(crate) fn prepare(conn: Connection) -> rusqlite::Result<Db> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA} COMMIT;"))?;
    }
    Ok(Db { conn })
}

/// Reads a TEXT column through `parse`, so a malformed value is an error rather than a panic.
fn text<T>(row: &Row, idx: usize, parse: impl Fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    parse(&s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            Type::Text,
            format!("unexpected {s:?}").into(),
        )
    })
}

// sqlite integers are i64. Milliseconds since 1970 fit until the year 292 million.
fn to_sql(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn from_sql(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

// Names in the settings table.
const ARTIFACT_DAYS: &str = "artifact_retention_days";
const LOG_DAYS: &str = "log_retention_days";

fn uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

impl Db {
    /// Records a new run of `pipeline` at `commit`, every task pending, except those `carried`
    /// over from an earlier run, by (stage, task) name.
    pub fn insert_run(
        &mut self,
        name: &str,
        pipeline: &Pipeline,
        commit: Option<Commit>,
        carried: &BTreeMap<(String, String), TaskRun>,
    ) -> rusqlite::Result<Run> {
        let tx = self.conn.transaction()?;
        let run = Run {
            id: Uuid::new_v4(),
            pipeline: name.to_string(),
            state: RunState::Running,
            stages: pipeline
                .stages
                .iter()
                .map(|stage| StageRun {
                    name: stage.name.clone(),
                    tasks: stage
                        .tasks
                        .iter()
                        .map(|task| {
                            let key = (stage.name.clone(), task.name.clone());
                            match carried.get(&key) {
                                Some(earlier) => TaskRun {
                                    id: Uuid::new_v4(),
                                    // A carried task points at the one that really ran.
                                    carried_from: Some(earlier.carried_from.unwrap_or(earlier.id)),
                                    ..earlier.clone()
                                },
                                None => TaskRun {
                                    id: Uuid::new_v4(),
                                    name: task.name.clone(),
                                    state: TaskState::Pending,
                                    worker: None,
                                    started: None,
                                    ended: None,
                                    carried_from: None,
                                    tests: None,
                                },
                            }
                        })
                        .collect(),
                })
                .collect(),
            commit,
        };

        let (sha, branch) = run.commit.as_ref().map(|c| (&c.sha, &c.branch)).unzip();
        tx.execute(
            "INSERT INTO pl_runs (id, pipeline, state, commit_sha, branch)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (run.id.to_string(), name, run.state.as_str(), sha, branch),
        )?;
        for stage in &run.stages {
            let stage_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO stage_runs (id, pl_run_id, name) VALUES (?1, ?2, ?3)",
                (&stage_id, run.id.to_string(), &stage.name),
            )?;
            for task in &stage.tasks {
                tx.execute(
                    "INSERT INTO task_runs
                     (id, stage_run_id, name, state, worker, started_at, ended_at, carried_from,
                      tests, tests_failed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    (
                        task.id.to_string(),
                        &stage_id,
                        &task.name,
                        task.state.as_str(),
                        &task.worker,
                        task.started.map(to_sql),
                        task.ended.map(to_sql),
                        task.carried_from.map(|id| id.to_string()),
                        task.tests.map(|t| to_sql(t.total)),
                        task.tests.map(|t| to_sql(t.failed)),
                    ),
                )?;
            }
        }
        tx.commit()?;
        Ok(run)
    }

    /// Sets a task's state at time `at`, which is when it ended if the state is a final one.
    pub fn set_task_state(&self, task: Uuid, state: TaskState, at: u64) -> rusqlite::Result<usize> {
        let ended = state.is_over().then(|| to_sql(at));
        self.conn.execute(
            "UPDATE task_runs SET state = ?1, ended_at = coalesce(?2, ended_at) WHERE id = ?3",
            (state.as_str(), ended, task.to_string()),
        )
    }

    /// Marks a task as running on `worker` since `at`.
    pub fn start_task(&self, task: Uuid, worker: &str, at: u64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE task_runs SET state = 'running', worker = ?1, started_at = ?2 WHERE id = ?3",
            (worker, to_sql(at), task.to_string()),
        )
    }

    pub fn set_tests(&self, task: Uuid, tests: Tests) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE task_runs SET tests = ?1, tests_failed = ?2 WHERE id = ?3",
            (to_sql(tests.total), to_sql(tests.failed), task.to_string()),
        )
    }

    pub fn set_run_state(&self, run: Uuid, state: RunState) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE pl_runs SET state = ?1 WHERE id = ?2",
            (state.as_str(), run.to_string()),
        )
    }

    /// In one transaction: committing each line on its own takes most of the time.
    pub fn append_log(&mut self, task: Uuid, lines: &[LogLine]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO log_lines (task_run_id, stream, line) VALUES (?1, ?2, ?3)",
        )?;
        let task = task.to_string();
        for line in lines {
            insert.execute((&task, line.stream.as_str(), &line.line))?;
        }
        drop(insert);
        tx.commit()
    }

    pub fn log(&self, task: Uuid) -> rusqlite::Result<Vec<LogLine>> {
        self.conn
            .prepare_cached(
                "SELECT stream, line FROM log_lines WHERE task_run_id = ?1 ORDER BY rowid",
            )?
            .query_map([task.to_string()], |row| {
                Ok(LogLine {
                    stream: text(row, 0, LogStream::parse)?,
                    line: row.get(1)?,
                })
            })?
            .collect()
    }

    pub fn task_state(&self, task: Uuid) -> rusqlite::Result<Option<TaskState>> {
        self.conn
            .prepare_cached("SELECT state FROM task_runs WHERE id = ?1")?
            .query_row([task.to_string()], |row| text(row, 0, TaskState::parse))
            .optional()
    }

    /// The newest run of `pipeline` in which task `stage.task` passed and published artifact
    /// `name`.
    pub fn latest_run_with_artifact(
        &self,
        pipeline: &str,
        stage: &str,
        task: &str,
        name: &str,
    ) -> rusqlite::Result<Option<Uuid>> {
        self.conn
            .prepare_cached(
                "SELECT pl_runs.id FROM pl_runs
                 JOIN stage_runs ON stage_runs.pl_run_id = pl_runs.id
                 JOIN task_runs ON task_runs.stage_run_id = stage_runs.id
                 JOIN task_artifacts ON task_artifacts.task_run_id = task_runs.id
                 WHERE pl_runs.pipeline = ?1 AND stage_runs.name = ?2 AND task_runs.name = ?3
                   AND task_runs.state = 'pass' AND task_artifacts.name = ?4
                 ORDER BY pl_runs.rowid DESC LIMIT 1",
            )?
            .query_row((pipeline, stage, task, name), |row| text(row, 0, uuid))
            .optional()
    }

    /// Runs of `pipeline` whose id, without dashes, starts with `prefix` (lowercase hex digits).
    /// At most `limit`, newest first.
    pub fn runs_by_prefix(
        &self,
        pipeline: &str,
        prefix: &str,
        limit: i64,
    ) -> rusqlite::Result<Vec<Uuid>> {
        self.conn
            .prepare_cached(
                "SELECT id FROM pl_runs WHERE pipeline = ?1 AND replace(id, '-', '') LIKE ?2 || '%'
                 ORDER BY rowid DESC LIMIT ?3",
            )?
            .query_map((pipeline, prefix, limit), |row| text(row, 0, uuid))?
            .collect()
    }

    pub fn add_artifact(&self, task: Uuid, artifact: &Artifact) -> rusqlite::Result<usize> {
        self.conn.execute(
            "INSERT INTO task_artifacts (task_run_id, name, size) VALUES (?1, ?2, ?3)",
            (task.to_string(), &artifact.name, to_sql(artifact.size)),
        )
    }

    /// The artifacts a task published, in the order the boss received them.
    pub fn artifacts(&self, task: Uuid) -> rusqlite::Result<Vec<Artifact>> {
        self.conn
            .prepare_cached(
                "SELECT name, size FROM task_artifacts WHERE task_run_id = ?1 ORDER BY rowid",
            )?
            .query_map([task.to_string()], |row| {
                Ok(Artifact {
                    name: row.get(0)?,
                    size: from_sql(row.get(1)?),
                })
            })?
            .collect()
    }

    pub fn run(&self, id: Uuid) -> rusqlite::Result<Option<Run>> {
        let Some((pipeline, state, (sha, branch))) = self
            .conn
            .prepare_cached(
                "SELECT pipeline, state, commit_sha, branch FROM pl_runs WHERE id = ?1",
            )?
            .query_row([id.to_string()], |row| {
                let commit: (Option<String>, Option<String>) = (row.get(2)?, row.get(3)?);
                Ok((row.get(0)?, text(row, 1, RunState::parse)?, commit))
            })
            .optional()?
        else {
            return Ok(None);
        };

        let stage_rows: Vec<(String, String)> = self
            .conn
            .prepare_cached("SELECT id, name FROM stage_runs WHERE pl_run_id = ?1 ORDER BY rowid")?
            .query_map([id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut stages = vec![];
        for (stage_id, name) in stage_rows {
            let tasks = self
                .conn
                .prepare_cached(
                    "SELECT id, name, state, worker, started_at, ended_at, carried_from, tests,
                     tests_failed FROM task_runs WHERE stage_run_id = ?1 ORDER BY rowid",
                )?
                .query_map([stage_id], |row| {
                    Ok(TaskRun {
                        id: text(row, 0, uuid)?,
                        name: row.get(1)?,
                        state: text(row, 2, TaskState::parse)?,
                        worker: row.get(3)?,
                        started: row.get::<_, Option<i64>>(4)?.map(from_sql),
                        ended: row.get::<_, Option<i64>>(5)?.map(from_sql),
                        carried_from: row.get::<_, Option<String>>(6)?.as_deref().and_then(uuid),
                        tests: Option::zip(row.get(7)?, row.get(8)?).map(|(total, failed)| Tests {
                            total: from_sql(total),
                            failed: from_sql(failed),
                        }),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            stages.push(StageRun { name, tasks });
        }

        Ok(Some(Run {
            id,
            pipeline,
            state,
            stages,
            commit: sha.zip(branch).map(|(sha, branch)| Commit { sha, branch }),
        }))
    }

    /// The newest `limit` runs of `pipeline`, newest first.
    pub fn runs(&self, pipeline: &str, limit: i64) -> rusqlite::Result<Vec<Run>> {
        let ids: Vec<Uuid> = self
            .conn
            .prepare_cached(
                "SELECT id FROM pl_runs WHERE pipeline = ?1 ORDER BY rowid DESC LIMIT ?2",
            )?
            .query_map((pipeline, limit), |row| text(row, 0, uuid))?
            .collect::<rusqlite::Result<_>>()?;

        ids.into_iter()
            .filter_map(|id| self.run(id).transpose())
            .collect()
    }

    /// The run a task belongs to, and the name of the task's stage.
    pub fn task(&self, id: Uuid) -> rusqlite::Result<Option<(Run, String)>> {
        let Some((run_id, stage)) = self
            .conn
            .prepare_cached(
                "SELECT stage_runs.pl_run_id, stage_runs.name FROM task_runs
                 JOIN stage_runs ON stage_runs.id = task_runs.stage_run_id
                 WHERE task_runs.id = ?1",
            )?
            .query_row([id.to_string()], |row| {
                Ok((text(row, 0, uuid)?, row.get(1)?))
            })
            .optional()?
        else {
            return Ok(None);
        };
        Ok(self.run(run_id)?.map(|run| (run, stage)))
    }

    pub fn settings(&self) -> rusqlite::Result<Settings> {
        let days = |name: &str| {
            let value: Option<String> = self
                .conn
                .prepare_cached("SELECT value FROM settings WHERE name = ?1")?
                .query_row([name], |row| row.get(0))
                .optional()?;
            Ok::<_, rusqlite::Error>(value.and_then(|v| v.parse().ok()))
        };
        Ok(Settings {
            artifact_days: days(ARTIFACT_DAYS)?,
            log_days: days(LOG_DAYS)?,
        })
    }

    pub fn set_settings(&mut self, settings: &Settings) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for (name, days) in [
            (ARTIFACT_DAYS, settings.artifact_days),
            (LOG_DAYS, settings.log_days),
        ] {
            match days {
                Some(days) => tx.execute(
                    "INSERT OR REPLACE INTO settings (name, value) VALUES (?1, ?2)",
                    (name, days.to_string()),
                )?,
                None => tx.execute("DELETE FROM settings WHERE name = ?1", [name])?,
            };
        }
        tx.commit()
    }

    /// Deletes the logs of up to `limit` tasks that ended before `cutoff` and marks them deleted
    /// at `now`. Returns how many tasks it dealt with.
    pub fn expire_logs(&mut self, cutoff: u64, now: u64, limit: i64) -> rusqlite::Result<usize> {
        let tx = self.conn.transaction()?;
        let ids: Vec<String> = tx
            .prepare_cached(
                "SELECT id FROM task_runs WHERE log_deleted_at IS NULL AND ended_at < ?1 LIMIT ?2",
            )?
            .query_map((to_sql(cutoff), limit), |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ids {
            tx.execute("DELETE FROM log_lines WHERE task_run_id = ?1", [id])?;
            tx.execute(
                "UPDATE task_runs SET log_deleted_at = ?1 WHERE id = ?2",
                (to_sql(now), id),
            )?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    /// Up to `limit` tasks that ended before `cutoff` and still have artifacts.
    pub fn expiring_artifacts(&self, cutoff: u64, limit: i64) -> rusqlite::Result<Vec<Uuid>> {
        self.conn
            .prepare_cached(
                "SELECT DISTINCT task_artifacts.task_run_id FROM task_artifacts
                 JOIN task_runs ON task_runs.id = task_artifacts.task_run_id
                 WHERE task_runs.ended_at < ?1 LIMIT ?2",
            )?
            .query_map((to_sql(cutoff), limit), |row| text(row, 0, uuid))?
            .collect()
    }

    /// The artifacts of `tasks` are gone, deleted by retention at `now`.
    pub fn forget_artifacts(&mut self, tasks: &[Uuid], now: u64) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for task in tasks {
            tx.execute(
                "DELETE FROM task_artifacts WHERE task_run_id = ?1",
                [task.to_string()],
            )?;
            tx.execute(
                "UPDATE task_runs SET artifacts_deleted_at = ?1 WHERE id = ?2",
                (to_sql(now), task.to_string()),
            )?;
        }
        tx.commit()
    }

    /// When retention deleted the log and the artifacts of a task, if it did.
    pub fn deleted(&self, task: Uuid) -> rusqlite::Result<(Option<u64>, Option<u64>)> {
        let deleted = self
            .conn
            .prepare_cached(
                "SELECT log_deleted_at, artifacts_deleted_at FROM task_runs WHERE id = ?1",
            )?
            .query_row([task.to_string()], |row| {
                let log: Option<i64> = row.get(0)?;
                let artifacts: Option<i64> = row.get(1)?;
                Ok((log.map(from_sql), artifacts.map(from_sql)))
            })
            .optional()?;
        Ok(deleted.unwrap_or_default())
    }

    /// Every user, the logged-out one first, with how many credentials of each kind they have.
    pub fn users(&self) -> rusqlite::Result<Vec<User>> {
        self.conn
            .prepare_cached(
                "SELECT id, name, role, id = 0,
                    (SELECT count(*) FROM credentials c WHERE c.user_id = u.id AND kind = 'password'),
                    (SELECT count(*) FROM credentials c WHERE c.user_id = u.id AND kind = 'certificate'),
                    (SELECT count(*) FROM credentials c WHERE c.user_id = u.id AND kind = 'passkey'),
                    (SELECT count(*) FROM api_tokens t WHERE t.user_id = u.id)
                 FROM users u ORDER BY id != 0, name",
            )?
            .query_map([], |row| {
                Ok(User {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: text(row, 2, Role::parse)?,
                    builtin: row.get(3)?,
                    passwords: row.get(4)?,
                    certificates: row.get(5)?,
                    passkeys: row.get(6)?,
                    tokens: row.get(7)?,
                    certs: vec![],
                    keys: vec![],
                })
            })?
            .collect::<rusqlite::Result<Vec<User>>>()
            .and_then(|mut users| {
                for user in &mut users {
                    user.certs = self.certificates(user.id)?;
                    user.keys = self.passkeys(user.id)?;
                }
                Ok(users)
            })
    }

    /// A user's client certificates, oldest first.
    pub fn certificates(&self, user: i64) -> rusqlite::Result<Vec<Certificate>> {
        self.conn
            .prepare_cached(
                "SELECT id, json_extract(data, '$.label'), json_extract(data, '$.expires')
                 FROM credentials WHERE user_id = ?1 AND kind = 'certificate' ORDER BY id",
            )?
            .query_map([user], |row| {
                Ok(Certificate {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    expires: row.get(2)?,
                })
            })?
            .collect()
    }

    /// Records a client certificate of `user` by its fingerprint: the boss keeps nothing else of it.
    pub fn add_certificate(
        &self,
        user: i64,
        label: &str,
        fingerprint: &str,
        expires: &str,
    ) -> rusqlite::Result<usize> {
        let data =
            serde_json::json!({ "label": label, "fingerprint": fingerprint, "expires": expires });
        self.conn.execute(
            "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'certificate', ?2)",
            (user, data.to_string()),
        )
    }

    /// Revokes a client certificate or a passkey: from now on it logs nobody in.
    pub fn revoke_credential(&self, credential: i64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM credentials WHERE id = ?1 AND kind IN ('certificate', 'passkey')",
            [credential],
        )
    }

    /// A user's passkeys, oldest first.
    pub fn passkeys(&self, user: i64) -> rusqlite::Result<Vec<Passkey>> {
        self.conn
            .prepare_cached(
                "SELECT id, json_extract(data, '$.label')
                 FROM credentials WHERE user_id = ?1 AND kind = 'passkey' ORDER BY id",
            )?
            .query_map([user], |row| {
                Ok(Passkey {
                    id: row.get(0)?,
                    label: row.get(1)?,
                })
            })?
            .collect()
    }

    /// Records a passkey of `user`: webauthn-rs's record of it (its credential id, public key and
    /// signature counter), as JSON, under `credential`, its id as the browser names it.
    pub fn add_passkey(
        &self,
        user: i64,
        label: &str,
        credential: &str,
        passkey: &serde_json::Value,
    ) -> rusqlite::Result<usize> {
        let data =
            serde_json::json!({ "label": label, "credential": credential, "passkey": passkey });
        self.conn.execute(
            "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'passkey', ?2)",
            (user, data.to_string()),
        )
    }

    /// The passkey with this credential id: its row, whose it is, and webauthn-rs's record of it.
    pub fn passkey(&self, credential: &str) -> rusqlite::Result<Option<(i64, i64, String)>> {
        self.conn
            .prepare_cached(
                "SELECT id, user_id, json_extract(data, '$.passkey') FROM credentials
                 WHERE kind = 'passkey' AND json_extract(data, '$.credential') = ?1",
            )?
            .query_row([credential], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
    }

    /// webauthn-rs's records of a user's passkeys, oldest first.
    pub fn user_passkeys(&self, user: i64) -> rusqlite::Result<Vec<String>> {
        self.conn
            .prepare_cached(
                "SELECT json_extract(data, '$.passkey') FROM credentials
                 WHERE user_id = ?1 AND kind = 'passkey' ORDER BY id",
            )?
            .query_map([user], |row| row.get(0))?
            .collect()
    }

    /// Replaces webauthn-rs's record of a passkey, whose signature counter moved on at a login.
    pub fn set_passkey(
        &self,
        credential: i64,
        passkey: &serde_json::Value,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE credentials SET data = json_set(data, '$.passkey', json(?2)) WHERE id = ?1",
            (credential, passkey.to_string()),
        )
    }

    /// The name of a user, for a passkey's label and the authenticator's prompt.
    pub fn user_name(&self, user: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .prepare_cached("SELECT name FROM users WHERE id = ?1 AND id != 0")?
            .query_row([user], |row| row.get(0))
            .optional()
    }

    /// The id, name and role of the user whose client certificate has this fingerprint.
    pub fn certificate_user(
        &self,
        fingerprint: &str,
    ) -> rusqlite::Result<Option<(i64, String, Role)>> {
        self.conn
            .prepare_cached(
                "SELECT users.id, users.name, users.role FROM credentials JOIN users ON users.id = credentials.user_id
                 WHERE credentials.kind = 'certificate'
                   AND json_extract(credentials.data, '$.fingerprint') = ?1",
            )?
            .query_row([fingerprint], |row| {
                Ok((row.get(0)?, row.get(1)?, text(row, 2, Role::parse)?))
            })
            .optional()
    }

    pub fn add_user(&self, name: &str, role: Role) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO users (name, role) VALUES (?1, ?2)",
            (name, role.as_str()),
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_role(&self, user: i64, role: Role) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE users SET role = ?1 WHERE id = ?2",
            (role.as_str(), user),
        )
    }

    /// Removes a user, with their credentials and sessions. The logged-out user stays.
    pub fn remove_user(&self, user: i64) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM users WHERE id = ?1 AND id != 0", [user])
    }

    /// Replaces a user's password (its Argon2id hash).
    pub fn set_password(&mut self, user: i64, hash: &str) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM credentials WHERE user_id = ?1 AND kind = 'password'",
            [user],
        )?;
        tx.execute(
            "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'password', ?2)",
            (user, hash),
        )?;
        tx.commit()
    }

    /// The id and password hash of the user called `name`, if they have a password.
    pub fn password(&self, name: &str) -> rusqlite::Result<Option<(i64, String)>> {
        self.conn
            .prepare_cached(
                "SELECT users.id, credentials.data FROM users
                 JOIN credentials ON credentials.user_id = users.id AND credentials.kind = 'password'
                 WHERE users.name = ?1 AND users.id != 0",
            )?
            .query_row([name], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
    }

    pub fn add_api_token(&self, token_hash: &str, user: i64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "INSERT INTO api_tokens (token_hash, user_id) VALUES (?1, ?2)",
            (token_hash, user),
        )
    }

    /// The id, name and role of the user an API token is for.
    pub fn api_token(&self, token_hash: &str) -> rusqlite::Result<Option<(i64, String, Role)>> {
        self.conn
            .prepare_cached(
                "SELECT users.id, users.name, users.role FROM api_tokens
                 JOIN users ON users.id = api_tokens.user_id WHERE api_tokens.token_hash = ?1",
            )?
            .query_row([token_hash], |row| {
                Ok((row.get(0)?, row.get(1)?, text(row, 2, Role::parse)?))
            })
            .optional()
    }

    pub fn revoke_api_tokens(&self, user: i64) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM api_tokens WHERE user_id = ?1", [user])
    }

    pub fn add_session(
        &self,
        token_hash: &str,
        user: i64,
        expires: u64,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            (token_hash, user, to_sql(expires)),
        )
    }

    /// The user of an unexpired session, as (name, role, expires).
    pub fn session(
        &self,
        token_hash: &str,
        now: u64,
    ) -> rusqlite::Result<Option<(i64, String, Role, u64)>> {
        self.conn
            .prepare_cached(
                "SELECT users.id, users.name, users.role, sessions.expires_at FROM sessions
                 JOIN users ON users.id = sessions.user_id
                 WHERE sessions.token_hash = ?1 AND sessions.expires_at > ?2",
            )?
            .query_row((token_hash, to_sql(now)), |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    text(row, 2, Role::parse)?,
                    from_sql(row.get(3)?),
                ))
            })
            .optional()
    }

    pub fn extend_session(&self, token_hash: &str, expires: u64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2",
            (to_sql(expires), token_hash),
        )
    }

    /// Ends one session, and every one that has expired by `now`.
    pub fn end_session(&self, token_hash: &str, now: u64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM sessions WHERE token_hash = ?1 OR expires_at <= ?2",
            (token_hash, to_sql(now)),
        )
    }

    /// The workers the boss keeps a connection to.
    pub fn workers(&self) -> rusqlite::Result<Vec<String>> {
        self.conn
            .prepare_cached("SELECT url FROM workers ORDER BY rowid")?
            .query_map([], |row| row.get(0))?
            .collect()
    }

    /// Adds a worker the boss keeps a connection to. Adding one twice is fine.
    pub fn add_worker(&self, url: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("INSERT OR IGNORE INTO workers (url) VALUES (?1)", [url])
    }

    pub fn forget_worker(&self, url: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM workers WHERE url = ?1", [url])?;
        self.conn
            .execute("DELETE FROM heartbeats WHERE who = ?1", [url])?;
        Ok(())
    }

    /// Records that `who` (`boss`, or a worker's url) was alive at `at`.
    pub fn heartbeat(&self, who: &str, at: u64) -> rusqlite::Result<usize> {
        self.conn.execute(
            "INSERT INTO heartbeats (who, at) VALUES (?1, ?2)
             ON CONFLICT (who) DO UPDATE SET at = excluded.at",
            (who, to_sql(at)),
        )
    }

    /// When `who` was last known to be alive.
    pub fn last_heartbeat(&self, who: &str) -> rusqlite::Result<Option<u64>> {
        self.conn
            .prepare_cached("SELECT at FROM heartbeats WHERE who = ?1")?
            .query_row([who], |row| Ok(from_sql(row.get(0)?)))
            .optional()
    }

    /// Runs still marked running when the boss starts were cut off when the previous boss process
    /// stopped. Nothing will ever finish them, so fail them instead of leaving them spinning
    /// forever. The boss's last heartbeat says when that was, so their tasks end then, and say so;
    /// `now` stands in for a boss that never wrote one. Returns how many runs there were.
    pub fn fail_abandoned_runs(&mut self, now: u64) -> rusqlite::Result<usize> {
        let stopped = to_sql(self.last_heartbeat("boss")?.unwrap_or(now));
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO log_lines (task_run_id, stream, line)
             SELECT id, 'bettertest', 'the boss stopped while this task was running: its last \
             heartbeat was at ' || strftime('%Y-%m-%d %H:%M:%S UTC', ?1 / 1000, 'unixepoch')
             FROM task_runs WHERE state = 'running'",
            [stopped],
        )?;
        tx.execute(
            "UPDATE task_runs SET state = 'fail', ended_at = max(?1, coalesce(started_at, ?1))
             WHERE state = 'running'",
            [stopped],
        )?;
        tx.execute(
            "UPDATE task_runs SET state = 'skipped' WHERE state = 'pending'",
            [],
        )?;
        let abandoned = tx.execute(
            "UPDATE pl_runs SET state = 'fail' WHERE state = 'running'",
            [],
        )?;
        tx.commit()?;
        Ok(abandoned)
    }
}

#[cfg(test)]
#[path = "../test/db.rs"]
mod tests;
