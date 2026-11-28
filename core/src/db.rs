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

/// A user's id, name and role, from the first three columns.
fn user(row: &Row) -> rusqlite::Result<(i64, String, Role)> {
    Ok((row.get(0)?, row.get(1)?, text(row, 2, Role::parse)?))
}

impl Db {
    /// The row `sql` finds, if any, as `read` reads it.
    fn one<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        read: impl FnOnce(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Option<T>> {
        self.conn
            .prepare_cached(sql)?
            .query_row(params, read)
            .optional()
    }

    /// Every row `sql` finds, as `read` reads it.
    fn all<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        read: impl FnMut(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Vec<T>> {
        self.conn
            .prepare_cached(sql)?
            .query_map(params, read)?
            .collect()
    }

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
                                None => TaskRun::pending(Uuid::new_v4(), task.name.clone()),
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
            let sql = "INSERT INTO stage_runs (id, pl_run_id, name) VALUES (?1, ?2, ?3)";
            tx.execute(sql, (&stage_id, run.id.to_string(), &stage.name))?;
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
        let sql =
            "UPDATE task_runs SET state = ?1, ended_at = coalesce(?2, ended_at) WHERE id = ?3";
        self.conn
            .execute(sql, (state.as_str(), ended, task.to_string()))
    }

    /// Marks a task as running on `worker` since `at`.
    pub fn start_task(&self, task: Uuid, worker: &str, at: u64) -> rusqlite::Result<usize> {
        let sql =
            "UPDATE task_runs SET state = 'running', worker = ?1, started_at = ?2 WHERE id = ?3";
        self.conn
            .execute(sql, (worker, to_sql(at), task.to_string()))
    }

    pub fn set_tests(&self, task: Uuid, tests: Tests) -> rusqlite::Result<usize> {
        let sql = "UPDATE task_runs SET tests = ?1, tests_failed = ?2 WHERE id = ?3";
        self.conn.execute(
            sql,
            (to_sql(tests.total), to_sql(tests.failed), task.to_string()),
        )
    }

    pub fn set_run_state(&self, run: Uuid, state: RunState) -> rusqlite::Result<usize> {
        let sql = "UPDATE pl_runs SET state = ?1 WHERE id = ?2";
        self.conn.execute(sql, (state.as_str(), run.to_string()))
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
        let sql = "SELECT stream, line FROM log_lines WHERE task_run_id = ?1 ORDER BY rowid";
        self.all(sql, [task.to_string()], |row| {
            Ok(LogLine {
                stream: text(row, 0, LogStream::parse)?,
                line: row.get(1)?,
            })
        })
    }

    pub fn task_state(&self, task: Uuid) -> rusqlite::Result<Option<TaskState>> {
        let sql = "SELECT state FROM task_runs WHERE id = ?1";
        self.one(sql, [task.to_string()], |row| {
            text(row, 0, TaskState::parse)
        })
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
        let sql = "SELECT pl_runs.id FROM pl_runs
             JOIN stage_runs ON stage_runs.pl_run_id = pl_runs.id
             JOIN task_runs ON task_runs.stage_run_id = stage_runs.id
             JOIN task_artifacts ON task_artifacts.task_run_id = task_runs.id
             WHERE pl_runs.pipeline = ?1 AND stage_runs.name = ?2 AND task_runs.name = ?3
               AND task_runs.state = 'pass' AND task_artifacts.name = ?4
             ORDER BY pl_runs.rowid DESC LIMIT 1";
        self.one(sql, (pipeline, stage, task, name), |row| text(row, 0, uuid))
    }

    /// Runs of `pipeline` whose id, without dashes, starts with `prefix` (lowercase hex digits).
    /// At most `limit`, newest first.
    pub fn runs_by_prefix(
        &self,
        pipeline: &str,
        prefix: &str,
        limit: i64,
    ) -> rusqlite::Result<Vec<Uuid>> {
        let sql =
            "SELECT id FROM pl_runs WHERE pipeline = ?1 AND replace(id, '-', '') LIKE ?2 || '%'
             ORDER BY rowid DESC LIMIT ?3";
        self.all(sql, (pipeline, prefix, limit), |row| text(row, 0, uuid))
    }

    pub fn add_artifact(&self, task: Uuid, artifact: &Artifact) -> rusqlite::Result<usize> {
        let sql = "INSERT INTO task_artifacts (task_run_id, name, size) VALUES (?1, ?2, ?3)";
        self.conn.execute(
            sql,
            (task.to_string(), &artifact.name, to_sql(artifact.size)),
        )
    }

    /// The artifacts a task published, in the order the boss received them.
    pub fn artifacts(&self, task: Uuid) -> rusqlite::Result<Vec<Artifact>> {
        let sql = "SELECT name, size FROM task_artifacts WHERE task_run_id = ?1 ORDER BY rowid";
        self.all(sql, [task.to_string()], |row| {
            Ok(Artifact {
                name: row.get(0)?,
                size: from_sql(row.get(1)?),
            })
        })
    }

    pub fn run(&self, id: Uuid) -> rusqlite::Result<Option<Run>> {
        let runs = self.runs_where(
            "SELECT rowid, * FROM pl_runs WHERE id = ?1",
            [id.to_string()],
        )?;
        Ok(runs.into_iter().next())
    }

    /// The newest `limit` runs of `pipeline`, newest first.
    pub fn runs(&self, pipeline: &str, limit: i64) -> rusqlite::Result<Vec<Run>> {
        self.runs_where(
            "SELECT rowid, * FROM pl_runs WHERE pipeline = ?1 ORDER BY rowid DESC LIMIT ?2",
            (pipeline, limit),
        )
    }

    /// The runs `chosen` picks out of pl_runs (its rows, with their rowid), newest first, with
    /// their stages and tasks: one query, one row per task, which the rows of a run's stages and
    /// tasks join onto. A run or stage with no tasks still has its row, with the task's NULL.
    fn runs_where(
        &self,
        chosen: &str,
        params: impl rusqlite::Params,
    ) -> rusqlite::Result<Vec<Run>> {
        let sql = format!(
            "SELECT r.id, r.pipeline, r.state, r.commit_sha, r.branch, s.id, s.name,
                    t.id, t.name, t.state, t.worker, t.started_at, t.ended_at, t.carried_from,
                    t.tests, t.tests_failed
             FROM ({chosen}) r
             LEFT JOIN stage_runs s ON s.pl_run_id = r.id
             LEFT JOIN task_runs t ON t.stage_run_id = s.id
             ORDER BY r.rowid DESC, s.rowid, t.rowid"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let mut rows = statement.query(params)?;
        let mut runs: Vec<Run> = vec![];
        // The stage the last row was of, so that the next row of it adds to it.
        let mut last_stage: Option<String> = None;
        while let Some(row) = rows.next()? {
            let id = text(row, 0, uuid)?;
            if runs.last().is_none_or(|run| run.id != id) {
                let (sha, branch): (Option<String>, Option<String>) = (row.get(3)?, row.get(4)?);
                runs.push(Run {
                    id,
                    pipeline: row.get(1)?,
                    state: text(row, 2, RunState::parse)?,
                    stages: vec![],
                    commit: sha.zip(branch).map(|(sha, branch)| Commit { sha, branch }),
                });
                last_stage = None;
            }
            let Some(run) = runs.last_mut() else { continue };
            let Some(stage) = row.get::<_, Option<String>>(5)? else {
                continue;
            };
            if last_stage.as_ref() != Some(&stage) {
                run.stages.push(StageRun {
                    name: row.get(6)?,
                    tasks: vec![],
                });
                last_stage = Some(stage);
            }
            let Some(stage) = run.stages.last_mut() else {
                continue;
            };
            if row.get::<_, Option<String>>(7)?.is_none() {
                continue;
            }
            stage.tasks.push(TaskRun {
                id: text(row, 7, uuid)?,
                name: row.get(8)?,
                state: text(row, 9, TaskState::parse)?,
                worker: row.get(10)?,
                started: row.get::<_, Option<i64>>(11)?.map(from_sql),
                ended: row.get::<_, Option<i64>>(12)?.map(from_sql),
                carried_from: row.get::<_, Option<String>>(13)?.as_deref().and_then(uuid),
                tests: Option::zip(row.get(14)?, row.get(15)?).map(|(total, failed)| Tests {
                    total: from_sql(total),
                    failed: from_sql(failed),
                }),
            });
        }
        Ok(runs)
    }

    /// The run a task belongs to, and the name of the task's stage.
    pub fn task(&self, id: Uuid) -> rusqlite::Result<Option<(Run, String)>> {
        let sql = "SELECT stage_runs.pl_run_id, stage_runs.name FROM task_runs
             JOIN stage_runs ON stage_runs.id = task_runs.stage_run_id
             WHERE task_runs.id = ?1";
        let found = self.one(sql, [id.to_string()], |row| {
            Ok((text(row, 0, uuid)?, row.get(1)?))
        })?;
        let Some((run_id, stage)) = found else {
            return Ok(None);
        };
        Ok(self.run(run_id)?.map(|run| (run, stage)))
    }

    pub fn settings(&self) -> rusqlite::Result<Settings> {
        let days = |name: &str| {
            let sql = "SELECT value FROM settings WHERE name = ?1";
            let value: Option<String> = self.one(sql, [name], |row| row.get(0))?;
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
            let sql = "UPDATE task_runs SET log_deleted_at = ?1 WHERE id = ?2";
            tx.execute(sql, (to_sql(now), id))?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    /// Up to `limit` tasks that ended before `cutoff` and still have artifacts.
    pub fn expiring_artifacts(&self, cutoff: u64, limit: i64) -> rusqlite::Result<Vec<Uuid>> {
        let sql = "SELECT DISTINCT task_artifacts.task_run_id FROM task_artifacts
             JOIN task_runs ON task_runs.id = task_artifacts.task_run_id
             WHERE task_runs.ended_at < ?1 LIMIT ?2";
        self.all(sql, (to_sql(cutoff), limit), |row| text(row, 0, uuid))
    }

    /// The artifacts of `tasks` are gone, deleted by retention at `now`.
    pub fn forget_artifacts(&mut self, tasks: &[Uuid], now: u64) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for task in tasks {
            let task = task.to_string();
            tx.execute("DELETE FROM task_artifacts WHERE task_run_id = ?1", [&task])?;
            let sql = "UPDATE task_runs SET artifacts_deleted_at = ?1 WHERE id = ?2";
            tx.execute(sql, (to_sql(now), task))?;
        }
        tx.commit()
    }

    /// When retention deleted the log and the artifacts of a task, if it did.
    pub fn deleted(&self, task: Uuid) -> rusqlite::Result<(Option<u64>, Option<u64>)> {
        let sql = "SELECT log_deleted_at, artifacts_deleted_at FROM task_runs WHERE id = ?1";
        let found = self.one(sql, [task.to_string()], |row| {
            let at = |i| row.get::<_, Option<i64>>(i).map(|at| at.map(from_sql));
            Ok((at(0)?, at(1)?))
        })?;
        Ok(found.unwrap_or_default())
    }

    /// Every user, the logged-out one first, with their credentials and how many API tokens they
    /// have.
    pub fn users(&self) -> rusqlite::Result<Vec<User>> {
        let sql = "SELECT id, name, role, id = 0,
                (SELECT count(*) FROM credentials c WHERE c.user_id = u.id AND kind = 'password'),
                (SELECT count(*) FROM api_tokens t WHERE t.user_id = u.id)
             FROM users u ORDER BY id != 0, name";
        self.all(sql, [], |row| {
            let (id, name, role) = user(row)?;
            Ok(User {
                id,
                name,
                role,
                builtin: row.get(3)?,
                passwords: row.get(4)?,
                tokens: row.get(5)?,
                certs: self.certificates(id)?,
                keys: self.passkeys(id)?,
            })
        })
    }

    /// A user's client certificates, oldest first.
    pub fn certificates(&self, user: i64) -> rusqlite::Result<Vec<Certificate>> {
        let sql = "SELECT id, json_extract(data, '$.label'), json_extract(data, '$.expires')
             FROM credentials WHERE user_id = ?1 AND kind = 'certificate' ORDER BY id";
        self.all(sql, [user], |row| {
            let (id, label, expires) = (row.get(0)?, row.get(1)?, row.get(2)?);
            Ok(Certificate { id, label, expires })
        })
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
        let sql = "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'certificate', ?2)";
        self.conn.execute(sql, (user, data.to_string()))
    }

    /// Revokes a client certificate or a passkey: from now on it logs nobody in.
    pub fn revoke_credential(&self, credential: i64) -> rusqlite::Result<usize> {
        let sql = "DELETE FROM credentials WHERE id = ?1 AND kind IN ('certificate', 'passkey')";
        self.conn.execute(sql, [credential])
    }

    /// A user's passkeys, oldest first.
    pub fn passkeys(&self, user: i64) -> rusqlite::Result<Vec<Passkey>> {
        let sql = "SELECT id, json_extract(data, '$.label')
             FROM credentials WHERE user_id = ?1 AND kind = 'passkey' ORDER BY id";
        self.all(sql, [user], |row| {
            let (id, label) = (row.get(0)?, row.get(1)?);
            Ok(Passkey { id, label })
        })
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
        let sql = "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'passkey', ?2)";
        self.conn.execute(sql, (user, data.to_string()))
    }

    /// The passkey with this credential id: its row, whose it is, and webauthn-rs's record of it.
    pub fn passkey(&self, credential: &str) -> rusqlite::Result<Option<(i64, i64, String)>> {
        let sql = "SELECT id, user_id, json_extract(data, '$.passkey') FROM credentials
             WHERE kind = 'passkey' AND json_extract(data, '$.credential') = ?1";
        self.one(sql, [credential], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    }

    /// webauthn-rs's records of a user's passkeys, oldest first.
    pub fn user_passkeys(&self, user: i64) -> rusqlite::Result<Vec<String>> {
        let sql = "SELECT json_extract(data, '$.passkey') FROM credentials
             WHERE user_id = ?1 AND kind = 'passkey' ORDER BY id";
        self.all(sql, [user], |row| row.get(0))
    }

    /// Replaces webauthn-rs's record of a passkey, whose signature counter moved on at a login.
    pub fn set_passkey(
        &self,
        credential: i64,
        passkey: &serde_json::Value,
    ) -> rusqlite::Result<usize> {
        let sql =
            "UPDATE credentials SET data = json_set(data, '$.passkey', json(?2)) WHERE id = ?1";
        self.conn.execute(sql, (credential, passkey.to_string()))
    }

    /// The name of a user, for a passkey's label and the authenticator's prompt.
    pub fn user_name(&self, user: i64) -> rusqlite::Result<Option<String>> {
        let sql = "SELECT name FROM users WHERE id = ?1 AND id != 0";
        self.one(sql, [user], |row| row.get(0))
    }

    /// The id, name and role of the user whose client certificate has this fingerprint.
    pub fn certificate_user(
        &self,
        fingerprint: &str,
    ) -> rusqlite::Result<Option<(i64, String, Role)>> {
        let sql = "SELECT users.id, users.name, users.role FROM credentials JOIN users ON users.id = credentials.user_id
             WHERE credentials.kind = 'certificate'
               AND json_extract(credentials.data, '$.fingerprint') = ?1";
        self.one(sql, [fingerprint], user)
    }

    pub fn add_user(&self, name: &str, role: Role) -> rusqlite::Result<i64> {
        let sql = "INSERT INTO users (name, role) VALUES (?1, ?2)";
        self.conn.execute(sql, (name, role.as_str()))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_role(&self, user: i64, role: Role) -> rusqlite::Result<usize> {
        let sql = "UPDATE users SET role = ?1 WHERE id = ?2";
        self.conn.execute(sql, (role.as_str(), user))
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
        let sql = "INSERT INTO credentials (user_id, kind, data) VALUES (?1, 'password', ?2)";
        tx.execute(sql, (user, hash))?;
        tx.commit()
    }

    /// The id and password hash of the user called `name`, if they have a password.
    pub fn password(&self, name: &str) -> rusqlite::Result<Option<(i64, String)>> {
        let sql = "SELECT users.id, credentials.data FROM users
             JOIN credentials ON credentials.user_id = users.id AND credentials.kind = 'password'
             WHERE users.name = ?1 AND users.id != 0";
        self.one(sql, [name], |row| Ok((row.get(0)?, row.get(1)?)))
    }

    pub fn add_api_token(&self, token_hash: &str, user: i64) -> rusqlite::Result<usize> {
        let sql = "INSERT INTO api_tokens (token_hash, user_id) VALUES (?1, ?2)";
        self.conn.execute(sql, (token_hash, user))
    }

    /// The id, name and role of the user an API token is for.
    pub fn api_token(&self, token_hash: &str) -> rusqlite::Result<Option<(i64, String, Role)>> {
        let sql = "SELECT users.id, users.name, users.role FROM api_tokens
             JOIN users ON users.id = api_tokens.user_id WHERE api_tokens.token_hash = ?1";
        self.one(sql, [token_hash], user)
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
        let sql = "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)";
        self.conn.execute(sql, (token_hash, user, to_sql(expires)))
    }

    /// The user of an unexpired session, as (name, role, expires).
    pub fn session(
        &self,
        token_hash: &str,
        now: u64,
    ) -> rusqlite::Result<Option<(i64, String, Role, u64)>> {
        let sql = "SELECT users.id, users.name, users.role, sessions.expires_at FROM sessions
             JOIN users ON users.id = sessions.user_id
             WHERE sessions.token_hash = ?1 AND sessions.expires_at > ?2";
        self.one(sql, (token_hash, to_sql(now)), |row| {
            let (id, name, role) = user(row)?;
            Ok((id, name, role, from_sql(row.get(3)?)))
        })
    }

    pub fn extend_session(&self, token_hash: &str, expires: u64) -> rusqlite::Result<usize> {
        let sql = "UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2";
        self.conn.execute(sql, (to_sql(expires), token_hash))
    }

    /// Ends one session, and every one that has expired by `now`.
    pub fn end_session(&self, token_hash: &str, now: u64) -> rusqlite::Result<usize> {
        let sql = "DELETE FROM sessions WHERE token_hash = ?1 OR expires_at <= ?2";
        self.conn.execute(sql, (token_hash, to_sql(now)))
    }

    /// The workers the boss keeps a connection to.
    pub fn workers(&self) -> rusqlite::Result<Vec<String>> {
        self.all("SELECT url FROM workers ORDER BY rowid", [], |row| {
            row.get(0)
        })
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
        let sql = "INSERT INTO heartbeats (who, at) VALUES (?1, ?2)
             ON CONFLICT (who) DO UPDATE SET at = excluded.at";
        self.conn.execute(sql, (who, to_sql(at)))
    }

    /// When `who` was last known to be alive.
    pub fn last_heartbeat(&self, who: &str) -> rusqlite::Result<Option<u64>> {
        let sql = "SELECT at FROM heartbeats WHERE who = ?1";
        self.one(sql, [who], |row| Ok(from_sql(row.get(0)?)))
    }

    /// The runs not over, oldest first. When the boss starts, those are the ones the previous boss
    /// process left going.
    pub fn running_runs(&self) -> rusqlite::Result<Vec<Uuid>> {
        let sql = "SELECT id FROM pl_runs WHERE state = 'running' ORDER BY rowid";
        self.all(sql, [], |row| text(row, 0, uuid))
    }

    /// Where run `id` comes in the order runs were started.
    pub fn run_order(&self, id: Uuid) -> rusqlite::Result<i64> {
        let sql = "SELECT rowid FROM pl_runs WHERE id = ?1";
        self.conn.query_row(sql, [id.to_string()], |row| row.get(0))
    }

    /// Puts a task back to pending, as if it had never run, but for its log: a resumed run runs it
    /// again. Its artifacts' files are the caller's to delete.
    pub fn rewind_task(&mut self, task: Uuid) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        let task = task.to_string();
        tx.execute("DELETE FROM task_artifacts WHERE task_run_id = ?1", [&task])?;
        tx.execute(
            "UPDATE task_runs SET state = 'pending', worker = NULL, started_at = NULL,
             ended_at = NULL, carried_from = NULL, tests = NULL, tests_failed = NULL
             WHERE id = ?1",
            [&task],
        )?;
        tx.commit()
    }
}

#[cfg(test)]
#[path = "../test/db.rs"]
mod tests;
