CREATE TABLE IF NOT EXISTS presence (
    player_id      INTEGER PRIMARY KEY,
    place_id       INTEGER NOT NULL,
    entered_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    metadata_json  TEXT,
    FOREIGN KEY (player_id) REFERENCES players(id),
    FOREIGN KEY (place_id) REFERENCES places(id)
);
