CREATE TABLE IF NOT EXISTS runs (
  id          TEXT PRIMARY KEY, -- UUID
  number      INTEGER NOT NULL UNIQUE,
  started_at  INTEGER NOT NULL,
  finished_at INTEGER
);

CREATE TABLE IF NOT EXISTS stages (
  id     TEXT PRIMARY KEY, -- UUID
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE, -- UUID
  name   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
  id          TEXT PRIMARY KEY, -- UUID
  stage_id    TEXT NOT NULL REFERENCES stages(id) ON DELETE CASCADE, -- UUID
  name        TEXT NOT NULL,
  state       TEXT NOT NULL CHECK (state IN ('pending', 'running', 'pass', 'fail', 'disabled')),
  log_output  TEXT NOT NULL DEFAULT '',
  started_at  INTEGER,
  finished_at INTEGER
);
