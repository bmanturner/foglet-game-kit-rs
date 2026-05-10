CREATE TABLE IF NOT EXISTS place_recall (
    player_id      INTEGER NOT NULL,
    place_id       INTEGER NOT NULL,
    first_seen_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    snapshot_json  TEXT,
    PRIMARY KEY (player_id, place_id),
    FOREIGN KEY (player_id) REFERENCES players(id),
    FOREIGN KEY (place_id) REFERENCES places(id)
);
