pub mod migrations;
pub mod models;
pub mod queries;

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use tracing::info;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;

        // WAL mode for concurrent reads
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        migrations::run(&conn)?;

        info!("Database opened at {}", path.display());
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("DB lock poisoned: {}", e))?;
        f(&conn)
    }
}
