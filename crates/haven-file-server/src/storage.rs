use anyhow::{Result, bail};
use sha2::{Sha256, Digest};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tracing::{info, warn};

/// Manages on-disk file storage for transfers.
///
/// Each transfer is stored as a single flat file at `{storage_dir}/{transfer_id}`.
/// Sequential writes maximize throughput on HDDs.
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub async fn new(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).await?;
        info!("File storage directory: {}", dir.display());
        Ok(Self { dir })
    }

    /// Path to the file for a given transfer.
    ///
    /// Validates that `transfer_id` contains no path separators or ".." sequences
    /// to prevent path traversal attacks.
    pub fn file_path(&self, transfer_id: &str) -> PathBuf {
        assert!(
            !transfer_id.contains("..")
                && !transfer_id.contains('/')
                && !transfer_id.contains('\\'),
            "transfer_id contains path traversal characters: {}",
            transfer_id
        );
        self.dir.join(transfer_id)
    }

    /// Pre-allocate a file of the given size (sparse file on supported FSes).
    pub async fn create_file(&self, transfer_id: &str, size: u64) -> Result<()> {
        let path = self.file_path(transfer_id);
        let file = fs::File::create(&path).await?;
        file.set_len(size).await?;
        Ok(())
    }

    /// Write a chunk at a specific byte offset and verify its SHA-256 hash.
    /// Returns the number of bytes written.
    pub async fn write_chunk(
        &self,
        transfer_id: &str,
        offset: u64,
        expected_sha256: &str,
        data: &[u8],
    ) -> Result<usize> {
        // Verify hash before writing
        let mut hasher = Sha256::new();
        hasher.update(data);
        let actual_hash = hex::encode(hasher.finalize());

        if actual_hash != expected_sha256 {
            bail!(
                "Chunk hash mismatch: expected {}, got {}",
                expected_sha256,
                actual_hash
            );
        }

        let path = self.file_path(transfer_id);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;

        Ok(data.len())
    }

    /// Delete a transfer's file from disk.
    pub async fn delete_file(&self, transfer_id: &str) -> Result<()> {
        let path = self.file_path(transfer_id);
        match fs::remove_file(&path).await {
            Ok(()) => {
                info!("Deleted file for transfer {}", transfer_id);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!("File for transfer {} already gone", transfer_id);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

}
