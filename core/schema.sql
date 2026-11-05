-- The boss's db. A new db gets this schema; one that has any tables is used as it is. Until
-- bettertest is released, a change here is a breaking one: delete the old db (see AGENTS.md).
--
-- Rows come back in insertion order (rowid): runs oldest first, stages and tasks in the order they
-- run. Times are milliseconds since 1970 by the boss's clock. UUIDs are TEXT.

CREATE TABLE pl_runs (
    id               TEXT PRIMARY KEY,
    pipeline         TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('running', 'pass', 'fail')),
    -- The commit a run tests, for pipelines that live in a git repo. NULL for pipedef files.
    commit_sha       TEXT,
    branch           TEXT
);
CREATE INDEX pl_runs_by_pipeline ON pl_runs(pipeline);

CREATE TABLE stage_runs (
    id               TEXT PRIMARY KEY,
    pl_run_id        TEXT NOT NULL REFERENCES pl_runs(id) ON DELETE CASCADE,
    name             TEXT NOT NULL
);
CREATE INDEX stage_runs_by_pl_run ON stage_runs(pl_run_id);

CREATE TABLE task_runs (
    id               TEXT PRIMARY KEY,
    stage_run_id     TEXT NOT NULL REFERENCES stage_runs(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('pending', 'running', 'pass', 'fail', 'skipped')),
    -- The worker it ran on, and when it started and ended. NULL until it does.
    worker           TEXT,
    started_at       INTEGER,
    ended_at         INTEGER,
    -- A rerun carries over the tasks it does not run again: the copy keeps the state, worker and
    -- times, and points at the task run that really ran, whose log and artifacts it shows.
    carried_from     TEXT,
    -- When retention deleted its log and its artifacts, so its page can say why they are gone.
    -- log_deleted_at is set on every task retention has dealt with, log or not.
    log_deleted_at   INTEGER,
    artifacts_deleted_at INTEGER
);
CREATE INDEX task_runs_by_stage_run ON task_runs(stage_run_id);
-- Only the tasks whose logs retention has yet to look at, so each sweep reads just those.
CREATE INDEX task_runs_keeping_logs ON task_runs(ended_at) WHERE log_deleted_at IS NULL;

-- One row per line, so appending is one insert no matter how long the log already is.
CREATE TABLE log_lines (
    task_run_id      TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    stream           TEXT NOT NULL CHECK (stream IN ('stdout', 'stderr', 'bettertest')),
    line             TEXT NOT NULL
);
CREATE INDEX log_lines_by_task_run ON log_lines(task_run_id);

-- Files tasks published with artifact(), each at /var/tmp/bettertest/task-artifacts/<task>/<name>.
CREATE TABLE task_artifacts (
    task_run_id      TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    size             INTEGER NOT NULL,
    PRIMARY KEY (task_run_id, name)
);

-- The workers the boss keeps a connection to: ones pipedefs named, and ones added on the workers
-- page.
CREATE TABLE workers (
    url              TEXT PRIMARY KEY
);

-- When the boss, and each worker, was last known to be alive. The boss writes its own every 15
-- seconds, and a worker's whenever the worker's /events stream says anything (its keep-alives come
-- every 15 seconds). Once one is gone, this says when it went.
CREATE TABLE heartbeats (
    who              TEXT PRIMARY KEY,   -- 'boss', or a worker's url
    at               INTEGER NOT NULL
);

-- The settings page's: retention, and so on.
CREATE TABLE settings (
    name             TEXT PRIMARY KEY,
    value            TEXT NOT NULL
);

-- Users, what they may do, and how they prove who they are. Row 0 is "logged out": the role of
-- anyone who has not logged in, admin to start with, until someone locks the boss down by
-- lowering it.
CREATE TABLE users (
    id               INTEGER PRIMARY KEY,
    name             TEXT NOT NULL UNIQUE,
    role             TEXT NOT NULL CHECK (role IN ('none', 'read', 'write', 'admin'))
);
INSERT INTO users (id, name, role) VALUES (0, 'logged out', 'admin');

-- One row per way a user can log in; a user may have several of a kind. `data` is the kind's own:
-- for a password, its Argon2id hash (PHC string).
CREATE TABLE credentials (
    id               INTEGER PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN ('password', 'certificate', 'passkey')),
    data             TEXT NOT NULL
);
CREATE INDEX credentials_by_user ON credentials(user_id);

-- A browser that logged in. The cookie holds the token; only its hash is kept.
CREATE TABLE sessions (
    token_hash       TEXT PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at       INTEGER NOT NULL
);

-- How a script or curl acts as a user, with Authorization: Bearer <token>. Like sessions, only the
-- token's hash is kept; the token is shown once, when it is made.
CREATE TABLE api_tokens (
    token_hash       TEXT PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX api_tokens_by_user ON api_tokens(user_id);
