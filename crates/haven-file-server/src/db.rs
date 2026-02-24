use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::info;

const READER_POOL_SIZE: usize = 4;

/// File server database with reader/writer split (same pattern as haven-db).
pub struct FileDb {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    reader_idx: AtomicUsize,
}

impl FileDb {
    pub fn open(path: &Path) -> Result<Self> {
        let writer = Connection::open(path)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;

        run_migrations(&writer)?;

        let mut readers = Vec::with_capacity(READER_POOL_SIZE);
        for _ in 0..READER_POOL_SIZE {
            let conn = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            readers.push(Mutex::new(conn));
        }

        info!(
            "File DB opened at {} (1 writer + {} readers)",
            path.display(),
            READER_POOL_SIZE
        );
        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            reader_idx: AtomicUsize::new(0),
        })
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let idx = self.reader_idx.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        let conn = self.readers[idx]
            .lock()
            .map_err(|e| anyhow::anyhow!("Reader lock poisoned: {}", e))?;
        f(&conn)
    }

    pub fn with_conn_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.writer
            .lock()
            .map_err(|e| anyhow::anyhow!("Writer lock poisoned: {}", e))?;
        f(&conn)
    }
}

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);"
    )?;

    let version: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))?;

    if version < 1 {
        info!("File DB: running migration v1 (initial schema)");
        conn.execute_batch(
            "
            CREATE TABLE transfers (
                id TEXT PRIMARY KEY,
                uploader_id TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                chunk_size INTEGER NOT NULL DEFAULT 4194304,
                chunk_count INTEGER NOT NULL,
                file_sha256 TEXT NOT NULL,
                bytes_received INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'uploading',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                expires_at TEXT
            );

            CREATE TABLE chunks (
                transfer_id TEXT NOT NULL REFERENCES transfers(id) ON DELETE CASCADE,
                chunk_index INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                byte_offset INTEGER NOT NULL,
                byte_length INTEGER NOT NULL,
                received INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (transfer_id, chunk_index)
            );

            INSERT INTO schema_version (version) VALUES (1);
            "
        )?;
    }

    Ok(())
}
