CREATE TABLE IF NOT EXISTS routes (
    id                 INTEGER PRIMARY KEY,
    from_place_id      INTEGER NOT NULL,
    to_place_id        INTEGER NOT NULL,
    kind               TEXT NOT NULL,
    requirements_json  TEXT,
    metadata_json      TEXT,
    created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (from_place_id) REFERENCES places(id),
    FOREIGN KEY (to_place_id) REFERENCES places(id)
);
