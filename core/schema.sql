CREATE TABLE IF NOT EXISTS pl_runs (
    id               TEXT PRIMARY KEY, -- UUID
    number           INTEGER NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS stage_runs (
    id               TEXT PRIMARY KEY, -- UUID
    pl_run_id        TEXT NOT NULL REFERENCES pl_runs(id) ON DELETE CASCADE, -- UUID
    name             TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_runs (
    id               TEXT PRIMARY KEY, -- UUID
    stage_run_id     TEXT NOT NULL REFERENCES stage_runs(id) ON DELETE CASCADE, -- UUID
    name             TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('pending', 'running', 'pass', 'fail', 'disabled')),
    logs             TEXT NOT NULL DEFAULT ''
);
