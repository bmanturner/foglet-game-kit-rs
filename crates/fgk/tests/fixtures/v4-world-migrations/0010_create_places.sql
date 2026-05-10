CREATE TABLE IF NOT EXISTS places (
    id              INTEGER PRIMARY KEY,
    key             TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    kind            TEXT NOT NULL,
    metadata_json   TEXT,
    created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
