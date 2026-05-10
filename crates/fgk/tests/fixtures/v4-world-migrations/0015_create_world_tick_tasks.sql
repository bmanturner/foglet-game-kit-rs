CREATE TABLE IF NOT EXISTS world_tick_tasks (
    key              TEXT PRIMARY KEY,
    last_run_at      TEXT,
    interval_seconds INTEGER NOT NULL CHECK (interval_seconds > 0),
    metadata_json    TEXT
);
