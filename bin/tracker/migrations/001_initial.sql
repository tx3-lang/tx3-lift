CREATE TABLE matches (
    id            INTEGER PRIMARY KEY,
    tx_hash       BLOB    NOT NULL,
    block_slot    INTEGER NOT NULL,
    block_hash    BLOB    NOT NULL,
    source_name   TEXT    NOT NULL,
    protocol_name TEXT    NOT NULL,
    tx_name       TEXT    NOT NULL,
    profile_name  TEXT    NOT NULL,
    lifted        TEXT    NOT NULL,
    matched_at    INTEGER NOT NULL,
    UNIQUE(tx_hash, source_name)
);

CREATE INDEX idx_matches_block ON matches(block_slot, block_hash);
CREATE INDEX idx_matches_source ON matches(source_name);

CREATE TABLE cursor (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    slot       INTEGER NOT NULL,
    block_hash BLOB    NOT NULL
);
