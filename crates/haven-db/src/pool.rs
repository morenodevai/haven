use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tracing::info;

/// Number of read-only connections in the reader pool.
const READER_POOL_SIZE: usize = 4;

/// Generic SQLite connection pool with reader/writer split.
///
/// Write operations go through a single `Mutex<Connection>` (the writer).
/// Read operations are distributed across a pool of read-only connections
/// using round-robin selection via `AtomicUsize`. WAL mode enables concurrent
/// reads even while a write is in progress.
///
/// Callers provide a migration function to initialize the schema on the writer
/// connection before the reader pool is created. This keeps migration logic
/// in the owning crate while sharing the pool mechanics.
pub struct DbPool {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    reader_idx: AtomicUsize,
}

impl DbPool {
    /// Open a database at `path`, run `migrate` on the writer, then create
    /// `READER_POOL_SIZE` read-only connections.
    pub fn open(path: &Path, label: &str, migrate: impl FnOnce(&Connection) -> Result<()>) -> Result<Self> {
        let writer = Connection::open(path)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;

        migrate(&writer)?;

        let mut readers = Vec::with_capacity(READER_POOL_SIZE);
        for _ in 0..READER_POOL_SIZE {
            let conn = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            readers.push(Mutex::new(conn));
        }

        info!("{} opened at {} (1 writer + {} readers)", label, path.display(), READER_POOL_SIZE);
        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            reader_idx: AtomicUsize::new(0),
        })
    }

    /// Acquire a read-only connection (round-robin from the pool).
    /// Used for SELECT queries that don't modify the database.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let idx = self.reader_idx.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        let conn = self
            .readers[idx]
            .lock()
            .map_err(|e| anyhow::anyhow!("Reader lock poisoned: {}", e))?;
        f(&conn)
    }

    /// Acquire the writer connection for INSERT/UPDATE/DELETE queries.
    pub fn with_conn_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .writer
            .lock()
            .map_err(|e| anyhow::anyhow!("Writer lock poisoned: {}", e))?;
        f(&conn)
    }
}
