-- Base player table required by v4 `presence` and `place_recall`
-- foreign keys when applying this fixture into a fresh DB.
CREATE TABLE IF NOT EXISTS players (
    id              INTEGER PRIMARY KEY,
    foglet_user_id  TEXT,
    handle          TEXT NOT NULL,
    role            TEXT NOT NULL DEFAULT 'user',
    security_level  INTEGER NOT NULL DEFAULT 50,
    first_seen_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    local_dev_key   TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_foglet_user_id
    ON players(foglet_user_id) WHERE foglet_user_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_local_dev_key
    ON players(local_dev_key) WHERE local_dev_key IS NOT NULL;
