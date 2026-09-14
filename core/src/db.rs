use bettertest_shared::*;
use rusqlite::Connection;
use std::path::Path;
use uuid::Uuid;

use std::error::Error;
use std::fs::create_dir_all;

const WORKDIR: &str = "/var/tmp/bettertest";
const DB_FILENAME: &str = "bt.db";
const SCHEMA_SQL: &str = include_str!("../schema.sql");

pub fn open() -> Result<Db, Box<dyn Error>> {
    create_dir_all(WORKDIR)?;
    let conn = Connection::open(Path::new(WORKDIR).join(DB_FILENAME))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(Db { conn })
}

#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn get_next_run_number(&self) -> Result<i64, Box<dyn Error>> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(number) + 1, 0) FROM pl_runs",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn insert_run(&mut self, pl_run: &PlRun) -> Result<(), Box<dyn Error>> {
        let tx = self.conn.transaction()?;

        tx.execute(
            "INSERT INTO pl_runs (id, number) VALUES (?1, ?2);",
            (pl_run.id.to_string(), pl_run.num),
        )?;

        for stage in &pl_run.stages {
            tx.execute(
                "INSERT INTO stage_runs (id, pl_run_id, name) VALUES (?1, ?2, ?3);",
                (stage.id.to_string(), pl_run.id.to_string(), &stage.name),
            )?;

            for task in &stage.tasks {
                tx.execute(
                    "INSERT INTO task_runs (id, stage_run_id, name, state) VALUES (?1, ?2, ?3, ?4);",
                    (
                        task.id.to_string(),
                        stage.id.to_string(),
                        &task.name,
                        task.state.to_string(),
                    ),
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn append_logs(&self, task_id: Uuid, logs: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "UPDATE task_runs SET logs = logs || ?1 WHERE id = ?2;",
            (logs, task_id.to_string()),
        )?;
        Ok(())
    }

    pub fn get_logs(&self, task_id: Uuid) -> Result<String, Box<dyn Error>> {
        Ok(self.conn.query_one(
            "SELECT logs FROM task_runs WHERE id = ?1;",
            [task_id.to_string()],
            |logs| logs.get(0),
        )?)
    }

    pub fn set_task_finished(&self, task_id: Uuid, pass: bool) -> Result<(), Box<dyn Error>> {
        let state = if pass { "pass" } else { "fail" };
        self.conn.execute(
            "UPDATE task_runs SET state = ?1 WHERE id = ?2",
            (state, task_id.to_string()),
        )?;
        Ok(())
    }

    pub fn set_task_running(&self, task_id: Uuid) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            "UPDATE task_runs SET state = ?1 WHERE id = ?2",
            ("running", task_id.to_string()),
        )?;
        Ok(())
    }

    pub fn get_pipeline_runs(&mut self) -> Result<Vec<PlRun>, Box<dyn Error>> {
        let tx = self.conn.transaction()?;
        // TODO: Once we add pagination it will be in this query
        let run_rows: Vec<(String, i64)> = tx
            .prepare("SELECT id, number FROM pl_runs ORDER BY number DESC")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .map(|r| r.unwrap())
            .collect();

        let mut pl_runs: Vec<PlRun> = vec![];
        for (pl_run_id, pl_run_num) in run_rows {
            let stage_rows: Vec<(String, String)> = tx
                .prepare("SELECT * FROM stage_runs WHERE pl_run_id = ?1;")?
                .query_map([&pl_run_id], |row| Ok((row.get(0)?, row.get(2)?)))?
                .map(|r| r.unwrap())
                .collect();

            let mut stage_runs: Vec<StageRun> = vec![];
            for (stage_id, stage_name) in stage_rows {
                let task_runs: Vec<TaskRun> = tx
                    .prepare("SELECT * FROM task_runs WHERE stage_run_id = ?1;")?
                    .query_map([&stage_id], |row| {
                        Ok((row.get(0)?, row.get(2)?, row.get(3)?))
                    })?
                    .map(|r| {
                        let (task_id, task_name, state): (String, String, String) = r.unwrap();
                        TaskRun {
                            id: Uuid::parse_str(&task_id).unwrap(),
                            name: task_name,
                            state: state.parse().unwrap(),
                        }
                    })
                    .collect();
                stage_runs.push(StageRun {
                    id: Uuid::parse_str(&stage_id)?,
                    name: stage_name,
                    tasks: task_runs,
                });
            }
            pl_runs.push(PlRun {
                id: Uuid::parse_str(&pl_run_id).unwrap(),
                num: pl_run_num,
                stages: stage_runs,
            });
        }

        tx.rollback()?; // Read-only so we don't need to commit
        Ok(pl_runs)
    }
}
