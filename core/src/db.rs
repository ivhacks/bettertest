use bettertest_shared_crate::*;
use rusqlite::{Connection, params};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const PATH: &str = "/opt/bettertest/bt.db";

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn parse_state(s: &str) -> TaskState {
    match s {
        "pending" => TaskState::Pending,
        "running" => TaskState::Running,
        "pass" => TaskState::Pass,
        "fail" => TaskState::Fail,
        other => panic!("unknown task state: {other}"),
    }
}

impl Db {
    pub fn open() -> Self {
        fs::create_dir_all("/opt/bettertest").unwrap();
        let conn = Connection::open(PATH).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../schema.sql"))
            .unwrap();
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    pub fn create_run(&self, pipeline: &Pipeline) -> Run {
        let run_id = Uuid::new_v4();
        let now = now_ms();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();

        // Insert run
        // 1 if no rows, highest number + 1 otherwise
        let number: i64 = tx
            .query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM runs", [], |row| {
                row.get(0)
            })
            .unwrap();
        tx.execute(
            "INSERT INTO runs (id, number, started_at) VALUES (?1, ?2, ?3)",
            params![run_id.to_string(), number, now],
        )
        .unwrap();

        // Insert stages within run
        let mut stages = Vec::new();
        for stage in &pipeline.stages {
            let stage_id = Uuid::new_v4();
            tx.execute(
                "INSERT INTO stages (id, run_id, name) VALUES (?1, ?2, ?3)",
                params![stage_id.to_string(), run_id.to_string(), stage.name],
            )
            .unwrap();

            // Insert tasks within each stage
            let mut tasks = Vec::new();
            for task in &stage.tasks {
                let task_id = Uuid::new_v4();
                tx.execute(
                    "INSERT INTO tasks (id, stage_id, name, state) VALUES (?1, ?2, ?3, 'pending')",
                    params![task_id.to_string(), stage_id.to_string(), task.name],
                )
                .unwrap();
                tasks.push(TaskRun {
                    id: task_id,
                    name: task.name.clone(),
                    state: TaskState::Pending,
                });
            }
            stages.push(StageRun {
                name: stage.name.clone(),
                tasks,
            });
        }
        tx.commit().unwrap();

        Run {
            id: run_id,
            number,
            active: true,
            stages,
        }
    }

    pub fn list_runs(&self) -> Vec<Run> {
        let conn = self.conn.lock().unwrap();
        let run_rows: Vec<(String, i64, Option<i64>)> = conn
            .prepare("SELECT id, number, finished_at FROM runs ORDER BY number DESC")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        let mut runs = Vec::new();
        for (id, number, finished_at) in run_rows {
            let stage_rows: Vec<(String, String)> = conn
                .prepare("SELECT id, name FROM stages WHERE run_id = ?1 ORDER BY rowid")
                .unwrap()
                .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let mut stages = Vec::new();
            for (stage_id, name) in stage_rows {
                let tasks = conn
                    .prepare("SELECT id, name, state FROM tasks WHERE stage_id = ?1 ORDER BY rowid")
                    .unwrap()
                    .query_map(params![stage_id], |row| {
                        let task_id: String = row.get(0)?;
                        let task_name: String = row.get(1)?;
                        let state: String = row.get(2)?;
                        Ok(TaskRun {
                            id: Uuid::parse_str(&task_id).unwrap(),
                            name: task_name,
                            state: parse_state(&state),
                        })
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
                stages.push(StageRun { name, tasks });
            }
            runs.push(Run {
                id: Uuid::parse_str(&id).unwrap(),
                number,
                active: finished_at.is_none(),
                stages,
            });
        }
        runs
    }

    pub fn set_task_running(&self, task_id: Uuid) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET state = 'running', started_at = ?1 WHERE id = ?2",
            params![now_ms(), task_id.to_string()],
        )
        .unwrap();
    }

    pub fn set_task_finished(&self, task_id: Uuid, passed: bool, log_output: &str) {
        let state = if passed { "pass" } else { "fail" };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET state = ?1, log_output = ?2, finished_at = ?3 WHERE id = ?4",
            params![state, log_output, now_ms(), task_id.to_string()],
        )
        .unwrap();
    }

    pub fn set_run_finished(&self, run_id: Uuid) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE runs SET finished_at = ?1 WHERE id = ?2",
            params![now_ms(), run_id.to_string()],
        )
        .unwrap();
    }
}
