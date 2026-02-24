use anyhow::{Result, bail};
use sha2::{Sha256, Digest};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
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
    pub fn file_path(&self, transfer_id: &str) -> PathBuf {
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
        file.flush().await?;

        Ok(data.len())
    }

    /// Read bytes from a transfer file at a given offset.
    #[allow(dead_code)]
    pub async fn read_range(
        &self,
        transfer_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let path = self.file_path(transfer_id);
        let mut file = fs::File::open(&path).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut buf = vec![0u8; length];
        let n = file.read_exact(&mut buf).await.map(|_| length).or_else(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                // partial read at end of available data
                Ok(0)
            } else {
                Err(e)
            }
        })?;
        buf.truncate(n);
        Ok(buf)
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

    /// Get the actual size of the file on disk.
    #[allow(dead_code)]
    pub async fn file_size(&self, transfer_id: &str) -> Result<u64> {
        let path = self.file_path(transfer_id);
        let metadata = fs::metadata(&path).await?;
        Ok(metadata.len())
    }

    /// List all transfer IDs that have files on disk.
    #[allow(dead_code)]
    pub async fn list_files(&self) -> Result<Vec<String>> {
        let mut entries = fs::read_dir(&self.dir).await?;
        let mut ids = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if let Some(name) = entry.file_name().to_str() {
                ids.push(name.to_string());
            }
        }
        Ok(ids)
    }

    /// Compute the full SHA-256 of a stored file.
    #[allow(dead_code)]
    pub async fn verify_full_hash(&self, transfer_id: &str) -> Result<String> {
        let path = self.file_path(transfer_id);
        let mut file = fs::File::open(&path).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 4 * 1024 * 1024]; // 4 MB read buffer
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }
}
