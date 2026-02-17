pub mod migrations;
pub mod models;
pub mod queries;

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use tracing::info;

/// Single-connection database wrapper.
///
/// Uses `Mutex` because `rusqlite::Connection` is `Send` but not `Sync`
/// (it contains internal `RefCell`s), so `RwLock` cannot be used.
/// WAL mode is still set for concurrent read performance at the SQLite level.
///
/// For true read concurrency, migrate to a connection pool (r2d2 + rusqlite)
/// with separate reader and writer connections.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // WAL mode for concurrent reads (benefits connection pools; with a single
        // connection we still get crash-safety improvements).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        migrations::run(&conn)?;

        info!("Database opened at {}", path.display());
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Acquire the connection for read-only queries (SELECT).
    /// Semantically distinct from `with_conn_mut` to ease future migration
    /// to a reader/writer pool pattern.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock poisoned: {}", e))?;
        f(&conn)
    }

    /// Acquire the connection for write queries (INSERT/UPDATE/DELETE).
    /// Currently identical to `with_conn` but will use a dedicated writer
    /// connection once a pool is introduced.
    pub fn with_conn_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock poisoned: {}", e))?;
        f(&conn)
    }
}
