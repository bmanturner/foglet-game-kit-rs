CREATE TABLE IF NOT EXISTS inventory_slots (
    id             INTEGER PRIMARY KEY,
    owner_kind     TEXT NOT NULL,
    owner_id       INTEGER NOT NULL,
    item_key       TEXT NOT NULL,
    quantity       INTEGER NOT NULL DEFAULT 0 CHECK (quantity >= 0),
    equilibrium    INTEGER,
    metadata_json  TEXT,
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
