use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use crate::error::Result;

const MIGRATIONS: &[(&str, &str)] = &[(
    "001_initial",
    include_str!("../migrations/001_initial.sql"),
)];

#[derive(Clone, Debug)]
pub struct Store {
    inner: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ChainPoint {
    pub slot: u64,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct MatchRow<'a> {
    pub tx_hash: &'a [u8],
    pub block_slot: u64,
    pub block_hash: &'a [u8],
    pub source_name: &'a str,
    pub protocol_name: &'a str,
    pub tx_name: &'a str,
    pub profile_name: &'a str,
    pub lifted_json: &'a str,
}

impl Store {
    /// Open or create a SQLite database file and run pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            let conn = Connection::open(&path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            run_migrations(&conn)?;
            Ok(conn)
        })
        .await??;

        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory store for tests.
    #[allow(dead_code)]
    pub async fn open_memory() -> Result<Self> {
        let conn = tokio::task::spawn_blocking(|| -> Result<Connection> {
            let conn = Connection::open_in_memory()?;
            run_migrations(&conn)?;
            Ok(conn)
        })
        .await??;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Read the persisted cursor, if any.
    pub async fn cursor(&self) -> Result<Option<ChainPoint>> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<ChainPoint>> {
            let conn = conn.blocking_lock();
            let row: Option<(i64, Vec<u8>)> = conn
                .query_row(
                    "SELECT slot, block_hash FROM cursor WHERE id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(row.and_then(|(slot, hash)| {
                let hash: [u8; 32] = hash.try_into().ok()?;
                Some(ChainPoint {
                    slot: slot as u64,
                    hash,
                })
            }))
        })
        .await?
    }

    /// Apply a batch of matches and update the cursor in a single transaction.
    /// Re-inserting the same `(tx_hash, source_name)` pair is a no-op.
    pub async fn apply_block(
        &self,
        cursor: ChainPoint,
        rows: Vec<OwnedMatchRow>,
    ) -> Result<usize> {
        let conn = self.inner.clone();
        let inserted = tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut conn = conn.blocking_lock();
            let tx = conn.transaction()?;
            let now = unix_secs();
            let mut inserted = 0usize;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR IGNORE INTO matches \
                     (tx_hash, block_slot, block_hash, source_name, protocol_name, \
                      tx_name, profile_name, lifted, matched_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )?;
                for row in &rows {
                    let n = stmt.execute(params![
                        row.tx_hash,
                        row.block_slot as i64,
                        row.block_hash,
                        row.source_name,
                        row.protocol_name,
                        row.tx_name,
                        row.profile_name,
                        row.lifted_json,
                        now,
                    ])?;
                    inserted += n;
                }
            }
            tx.execute(
                "INSERT INTO cursor (id, slot, block_hash) VALUES (1, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET slot = excluded.slot, block_hash = excluded.block_hash",
                params![cursor.slot as i64, cursor.hash.to_vec()],
            )?;
            tx.commit()?;
            Ok(inserted)
        })
        .await??;
        Ok(inserted)
    }

    /// Delete all matches for a given tx hash (rollback path) and rewind the
    /// cursor to the supplied parent point.
    pub async fn undo_tx(&self, tx_hash: Vec<u8>, parent: Option<ChainPoint>) -> Result<usize> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut conn = conn.blocking_lock();
            let tx = conn.transaction()?;
            let deleted =
                tx.execute("DELETE FROM matches WHERE tx_hash = ?", params![tx_hash])?;
            match parent {
                Some(p) => {
                    tx.execute(
                        "INSERT INTO cursor (id, slot, block_hash) VALUES (1, ?, ?) \
                         ON CONFLICT(id) DO UPDATE SET slot = excluded.slot, block_hash = excluded.block_hash",
                        params![p.slot as i64, p.hash.to_vec()],
                    )?;
                }
                None => {
                    tx.execute("DELETE FROM cursor WHERE id = 1", [])?;
                }
            }
            tx.commit()?;
            Ok(deleted)
        })
        .await?
    }
}

#[derive(Debug, Clone)]
pub struct OwnedMatchRow {
    pub tx_hash: Vec<u8>,
    pub block_slot: u64,
    pub block_hash: Vec<u8>,
    pub source_name: String,
    pub protocol_name: String,
    pub tx_name: String,
    pub profile_name: String,
    pub lifted_json: String,
}

impl<'a> From<MatchRow<'a>> for OwnedMatchRow {
    fn from(r: MatchRow<'a>) -> Self {
        Self {
            tx_hash: r.tx_hash.to_vec(),
            block_slot: r.block_slot,
            block_hash: r.block_hash.to_vec(),
            source_name: r.source_name.to_string(),
            protocol_name: r.protocol_name.to_string(),
            tx_name: r.tx_name.to_string(),
            profile_name: r.profile_name.to_string(),
            lifted_json: r.lifted_json.to_string(),
        }
    }
}

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_versions (
            name TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
    )?;

    for (name, sql) in MIGRATIONS {
        let already: bool = conn
            .query_row(
                "SELECT 1 FROM _schema_versions WHERE name = ?",
                params![name],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if already {
            continue;
        }

        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO _schema_versions (name, applied_at) VALUES (?, ?)",
            params![name, unix_secs()],
        )?;
    }
    Ok(())
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
