# Tracker database

The tracker persists every matched transaction into a SQLite file
(default: `./tracker.db`, configurable via `[storage].database_path`).
This document describes the schema, the invariants that hold across
restarts and rollbacks, and a handful of queries you'll typically reach
for when consuming the data.

## Schema

The schema lives in `bin/tracker/migrations/001_initial.sql`. A tiny
inline runner at startup applies any unapplied migration in lex order
and tracks state in a `_schema_versions` bookkeeping table.

```sql
-- Bookkeeping for the migration runner. One row per applied migration.
CREATE TABLE _schema_versions (
    name        TEXT PRIMARY KEY,
    applied_at  INTEGER NOT NULL          -- unix seconds
);

-- One row per (tx_hash, source_name) pair the matcher accepted.
CREATE TABLE matches (
    id            INTEGER PRIMARY KEY,
    tx_hash       BLOB    NOT NULL,        -- 32 bytes, raw
    block_slot    INTEGER NOT NULL,        -- absolute slot of containing block
    block_hash    BLOB    NOT NULL,        -- 32 bytes, raw
    source_name   TEXT    NOT NULL,        -- from [[sources]].name in tracker.toml
    protocol_name TEXT    NOT NULL,        -- from the TII's `protocol.name`
    tx_name       TEXT    NOT NULL,        -- the matched `tx <name>(…)` declaration
    profile_name  TEXT    NOT NULL,        -- from [[sources]].profile
    lifted        TEXT    NOT NULL,        -- JSON-encoded `Lifted` (annotations)
    matched_at    INTEGER NOT NULL,        -- unix seconds when the row was inserted
    UNIQUE(tx_hash, source_name)
);

CREATE INDEX idx_matches_block  ON matches(block_slot, block_hash);
CREATE INDEX idx_matches_source ON matches(source_name);

-- Single-row table holding the last successfully-applied block.
CREATE TABLE cursor (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    slot        INTEGER NOT NULL,
    block_hash  BLOB    NOT NULL
);
```

### Why those columns

- **`(tx_hash, source_name)`** is the natural identity. A transaction
  may be relevant to several configured sources at once (different
  TIIs); each match becomes its own row. Re-applying the same block on
  a restart or after a transient stream re-subscribe is a no-op thanks
  to the `UNIQUE` constraint and the `INSERT OR IGNORE` issued by the
  store.
- **`block_slot` + `block_hash`** are denormalized onto every match so
  a rollback can unscope by block without joining anything. They also
  serve the most common dashboard query — "show me what landed in
  this slot range".
- **`lifted`** is a JSON string rather than a structured payload. The
  `Lifted` shape evolves with the lifter; storing the serialized form
  keeps schema migrations decoupled from annotation-shape changes, and
  SQLite's JSON1 functions (`json_extract`, `->`, `->>`) make field
  access ergonomic.
- **`matched_at`** records ingestion time, *not* block time. Use
  `block_slot` to derive on-chain time when needed.
- **`cursor`** is the resume point — the highest block whose matches
  have all been committed. Apply and undo paths update it inside the
  same transaction as the row writes, so the cursor is never ahead of
  what's persisted.

### Invariants

- Every row in `matches` belongs to a block whose `block_slot` and
  `block_hash` are also somewhere on the canonical chain *as the
  upstream sees it*. On `Undo`, the daemon deletes every row whose
  `tx_hash` matches the undone transaction and rewinds the cursor to
  the parent block. There is no soft-delete.
- The `cursor` table contains either zero rows (fresh DB, never run)
  or exactly one (id = 1).
- After a clean restart, the daemon resumes by passing
  `cursor.slot` + `cursor.block_hash` as the WatchTx intersect, so the
  upstream re-sends every event from that point forward. The
  `(tx_hash, source_name)` uniqueness makes the re-application
  idempotent.

## Common queries

All queries below assume `sqlite3 tracker.db` from the directory the
daemon runs in.

### Cursor state

```sql
SELECT slot, hex(block_hash) AS block_hash FROM cursor;
```

```text
slot       block_hash
---------- ----------------------------------------------------------------
186188550  a18369937e9acd0b0095173cbd35b8b2f013ba13dcc34518e6d5a8a5004433e1
```

### Recent matches

```sql
SELECT
    block_slot,
    hex(tx_hash)  AS tx,
    source_name,
    tx_name
FROM matches
ORDER BY block_slot DESC, id DESC
LIMIT 10;
```

### How many matches per source

```sql
SELECT source_name, count(*) AS rows
FROM matches
GROUP BY source_name
ORDER BY rows DESC;
```

### One specific tx, all sources that picked it up

```sql
SELECT source_name, tx_name, protocol_name, profile_name, matched_at
FROM matches
WHERE tx_hash = X'1E31B36253043A89A388714F7245156788CBDBCEED5D74EE81679E56A7B81A86';
```

(BLOB literals are written as `X'…'` with hex content, no `0x` prefix.)

### Matches inside a slot range

```sql
SELECT block_slot, hex(tx_hash), source_name
FROM matches
WHERE block_slot BETWEEN 186188000 AND 186200000
ORDER BY block_slot;
```

The `idx_matches_block(block_slot, block_hash)` index covers this lookup.

### Pull a specific party out of every match

```sql
SELECT
    block_slot,
    hex(tx_hash) AS tx,
    json_extract(lifted, '$.parties.burner.address') AS burner_addr_bytes,
    json_extract(lifted, '$.parties.burner.role')    AS burner_role
FROM matches
WHERE source_name = 'orcfax-burn-mainnet'
ORDER BY block_slot DESC
LIMIT 5;
```

The `address` field stores raw payment-address bytes as a JSON array
of integers (one per byte, including the network/header byte). To get
hex out, cast in your application — SQLite has no built-in
JSON-array-of-ints → bytes converter.

### Aggregate burn quantities for one policy

```sql
WITH burns AS (
    SELECT json_each.value AS b
    FROM matches, json_each(json_extract(lifted, '$.burns'))
    WHERE source_name = 'orcfax-burn-mainnet'
)
SELECT
    sum(CAST(json_extract(b, '$.assets[0][1]') AS INTEGER)) AS total_burned
FROM burns;
```

### Find tx that mints a specific token

```sql
SELECT block_slot, hex(tx_hash)
FROM matches
WHERE EXISTS (
    SELECT 1
    FROM json_each(json_extract(lifted, '$.mints')) m
    WHERE json_extract(m.value, '$.policy_name') = 'usda'
);
```

### Manual undo (one tx)

The daemon does this for you on `Undo` events. If you ever need to
prune a specific match by hand:

```sql
BEGIN;
DELETE FROM matches WHERE tx_hash = X'…';
-- only if you also want to reposition the resume point:
UPDATE cursor SET slot = ?, block_hash = X'…' WHERE id = 1;
COMMIT;
```

## Example row

The orcfax-burn demo (`bin/tracker/examples/orcfax-burn/`) produces a
row like the one below after replaying its pinned intersect against a
mainnet utxorpc server. Hex is shown for blob columns; the `lifted`
JSON has been pretty-printed and the byte-array `address` fields
collapsed to hex for readability.

```text
id            = 1
tx_hash       = 0x1E31B36253043A89A388714F7245156788CBDBCEED5D74EE81679E56A7B81A86
block_slot    = 186188550
block_hash    = 0xA18369937E9ACD0B0095173CBD35B8B2F013BA13DCC34518E6D5A8A5004433E1
source_name   = orcfax-burn-mainnet
protocol_name = orcfax-burn
tx_name       = burn_orcfax
profile_name  = mainnet
matched_at    = 1746302164          -- 2026-05-03 12:36:04 UTC
```

```jsonc
// lifted (annotations, condensed; all `address` arrays shown as hex):
{
  "tx_id":         "1e31b36253043a89a388714f7245156788cbdbceed5d74ee81679e56a7b81a86",
  "protocol_name": "orcfax-burn",
  "tx_name":       "burn_orcfax",
  "profile_name":  "mainnet",
  "tir_hash":      0,                // pre-existing bug; tracked separately
  "parties": {
    "burner": {
      "name": "burner",
      "address": "71193ee65211bb3b4e0ea5f751f415269355a650e2e3706f625cdf1a4b",
      "role": "Input"
    },
    "recipient": {
      "name": "recipient",
      "address": "613c12f6735ef87655c5b27bced3f828d857d0a27fd20f2cda18ebf2fb",
      "role": "Multiple"
    }
  },
  "inputs": [
    {
      "tir_input_name": "source",
      "utxo_ref":       ["3661546efd3f5c79319ea62d075b209933ee36424423 04a5592cfc1f36ff5735", 1],
      "address":        "71193ee65211bb3b4e0ea5f751f415269355a650e2e3706f625cdf1a4b",
      "party":          "burner",
      "assets":         { "Naked": 1392130 },
      "datum": {
        "raw":     "<80B inline-datum CBOR>",
        "decoded": {
          "Struct": {
            "constructor": 121,
            "fields": [
              { "Struct": { "constructor": 121, "fields": [
                  { "Bytes":  "CER/STRIKE-ADA/3" },
                  { "Number": 1777743623232 },
                  { "Struct": { "constructor": 121, "fields": [
                      { "Number": 2662492851 },
                      { "Number": 1250000000 }
                  ] } }
              ] } },
              { "Struct": { "constructor": 121, "fields": [
                  { "Bytes": "3c12f6735ef87655c5b27bced3f828d857d0a27fd20f2cda18ebf2fb" }
              ] } }
            ]
          }
        }
      }
    }
  ],
  "references": [],
  "outputs": [
    {
      "tir_output_index": 0,
      "address":          "613c12f6735ef87655c5b27bced3f828d857d0a27fd20f2cda18ebf2fb",
      "party":            "recipient",
      "assets":           { "Naked": 6457855811 },
      "datum":            null
    }
  ],
  "mints": [],
  "burns": [
    {
      "tir_mint_index": 0,
      "policy":         "193ee65211bb3b4e0ea5f751f415269355a650e2e3706f625cdf1a4b",
      "policy_name":    null,
      "assets":         [["", -25]],
      "redeemer":       null
    }
  ],
  "policies": {},
  "signers": [
    {
      "key_hash": "3c12f6735ef87655c5b27bced3f828d857d0a27fd20f2cda18ebf2fb",
      "party":    "recipient"
    }
  ],
  "metadata": []
}
```

The transaction is an Orcfax oracle retraction: the script consumes 26
inputs, all of which carry a single oracle datum (price feed
`CER/STRIKE-ADA/3`, a timestamp, and a rational price), burns 25
copies of the policy's default-name asset, and forwards the residual
ada to the publisher's payment address. The lifter reconstructs all of
this from the on-chain CBOR — no extra side data is consulted.

## Operational notes

- **WAL mode** is enabled at startup (`PRAGMA journal_mode=WAL`) so
  read-only consumers can poll the file while the daemon writes. The
  `tracker.db-wal` and `tracker.db-shm` files are normal artifacts of
  WAL mode and gitignored.
- **Concurrency**: the daemon serializes all writes through a single
  `tokio::sync::Mutex<Connection>`. Read-only consumers should open
  their own connections.
- **Retention** is unbounded; nothing prunes old rows. If long-lived
  daemons grow the file, add a periodic `DELETE FROM matches WHERE
  block_slot < ?` from outside.
- **Schema evolution**: new columns or tables go in
  `bin/tracker/migrations/00N_<name>.sql`. The runner picks them up by
  filename order on next startup.
