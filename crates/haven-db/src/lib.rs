pub mod migrations;
pub mod models;
pub mod pool;
pub mod queries;

use anyhow::Result;
use std::path::Path;

pub use pool::DbPool;

/// Gateway database — wraps DbPool with gateway-specific migrations.
pub struct Database {
    pool: DbPool,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let pool = DbPool::open(path, "Database", |conn| migrations::run(conn))?;
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
