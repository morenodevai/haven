use anyhow::Result;
use std::path::Path;
use tracing::info;

use haven_db::DbPool;

/// File server database — wraps DbPool with file-server-specific migrations.
pub struct FileDb {
    pool: DbPool,
}

impl FileDb {
    pub fn open(path: &Path) -> Result<Self> {
        let pool = DbPool::open(path, "File DB", run_migrations)?;
        Ok(Self { pool })
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T>,
    {
        self.pool.with_conn(f)
    }

    pub fn with_conn_mut<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T>,
    {
        self.pool.with_conn_mut(f)
    }
}

fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
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
