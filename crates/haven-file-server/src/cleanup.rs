use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::db::FileDb;
use crate::storage::Storage;

/// Background task that prunes expired transfers.
///
/// Runs on an interval, finds transfers past their `expires_at` timestamp,
/// deletes their files from disk, and marks them as expired in the DB.
pub async fn run_cleanup_loop(db: Arc<FileDb>, storage: Arc<Storage>, interval_secs: u64) {
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

    loop {
        interval.tick().await;

        match cleanup_expired(&db, &storage).await {
            Ok(count) => {
                if count > 0 {
                    info!("Cleanup: pruned {} expired transfers", count);
                }
            }
            Err(e) => {
                warn!("Cleanup error: {}", e);
            }
        }
    }
}

async fn cleanup_expired(db: &FileDb, storage: &Storage) -> anyhow::Result<usize> {
    // Find expired transfers
    let expired: Vec<String> = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM transfers
             WHERE expires_at IS NOT NULL
               AND expires_at < datetime('now')
               AND status != 'expired'"
        )?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    })?;

    let count = expired.len();
    for id in &expired {
        // Delete file from disk
        storage.delete_file(id).await.ok();

        // Mark as expired in DB
        db.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE transfers SET status = 'expired' WHERE id = ?1",
                [id],
            )?;
            Ok(())
        })?;
    }

    Ok(count)
}
