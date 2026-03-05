use crate::models::{FileRow, MessageRow, PendingFolderOfferRow, PendingOfferRow, ReactionRow, UserRow};
use crate::Database;
use anyhow::Result;
use rusqlite::Connection;

impl Database {
    // -- Users --

    pub fn create_user(&self, id: &str, username: &str, password_hash: &str) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO users (id, username, password) VALUES (?1, ?2, ?3)",
                (id, username, password_hash),
            )?;
            Ok(())
        })
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<UserRow>> {
        self.with_conn(|conn| query_user_by_username(conn, username))
    }

    // #18: get_user_by_id removed — unused in the codebase.

    // -- Messages --

    pub fn insert_message(
        &self,
        id: &str,
        channel_id: &str,
        author_id: &str,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO messages (id, channel_id, author_id, ciphertext, nonce) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, channel_id, author_id, ciphertext, nonce],
            )?;
            Ok(())
        })
    }

    /// #11: Get messages with optional cursor-based pagination.
    /// When `before` is provided, only messages with `created_at < before` are returned.
    pub fn get_messages(&self, channel_id: &str, limit: u32, before: Option<&str>) -> Result<Vec<MessageRow>> {
        self.with_conn(|conn| query_messages(conn, channel_id, limit, before))
    }

    pub fn get_username_by_id(&self, id: &str) -> Result<String> {
        self.with_conn(|conn| {
            conn.query_row("SELECT username FROM users WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(|_| anyhow::anyhow!("User not found: {}", id))
        })
    }

    // -- Reactions --

    /// Toggle a reaction: removes if exists, inserts if not.
    /// Returns (added, Option<id>) — added=true means inserted, added=false means removed.
    pub fn toggle_reaction(
        &self,
        id: &str,
        message_id: &str,
        user_id: &str,
        emoji: &str,
    ) -> Result<(bool, Option<String>)> {
        self.with_conn_mut(|conn| {
            // Check if reaction already exists
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
                    rusqlite::params![message_id, user_id, emoji],
                    |row| row.get(0),
                )
                .optional()?;

            if let Some(existing_id) = existing {
                // Remove existing reaction
                conn.execute("DELETE FROM reactions WHERE id = ?1", [&existing_id])?;
                Ok((false, Some(existing_id)))
            } else {
                // Insert new reaction
                conn.execute(
                    "INSERT INTO reactions (id, message_id, user_id, emoji) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![id, message_id, user_id, emoji],
                )?;
                Ok((true, Some(id.to_string())))
            }
        })
    }

    /// #34: Check if a message belongs to a specific channel.
    pub fn message_belongs_to_channel(&self, message_id: &str, channel_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE id = ?1 AND channel_id = ?2",
                rusqlite::params![message_id, channel_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    // -- Files --

    pub fn insert_file(&self, id: &str, uploader_id: &str, filename: &str, size: i64) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO files (id, uploader_id, filename, size) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, uploader_id, filename, size],
            )?;
            Ok(())
        })
    }

    pub fn get_file(&self, id: &str) -> Result<Option<FileRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, uploader_id, filename, size, created_at FROM files WHERE id = ?1",
            )?;
            let row = stmt
                .query_row([id], |row| {
                    Ok(FileRow {
                        id: row.get(0)?,
                        uploader_id: row.get(1)?,
                        filename: row.get(2)?,
                        size: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })
                .optional()?;
            Ok(row)
        })
    }

    // -- Pending Offers --

    pub fn insert_pending_offer(
        &self,
        transfer_id: &str,
        from_user_id: &str,
        to_user_id: &str,
        filename: &str,
        file_size: i64,
        file_sha256: Option<&str>,
        chunk_hashes: Option<&str>,
        file_server_url: Option<&str>,
        folder_id: Option<&str>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO pending_offers
                 (transfer_id, from_user_id, to_user_id, filename, file_size, file_sha256, chunk_hashes, file_server_url, folder_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![transfer_id, from_user_id, to_user_id, filename, file_size, file_sha256, chunk_hashes, file_server_url, folder_id],
            )?;
            Ok(())
        })
    }

    pub fn update_pending_offer_status(&self, transfer_id: &str, status: &str) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE pending_offers SET status = ?1 WHERE transfer_id = ?2",
                rusqlite::params![status, transfer_id],
            )?;
            Ok(())
        })
    }

    pub fn update_pending_offer_hashes(
        &self,
        transfer_id: &str,
        file_sha256: &str,
        chunk_hashes: &str,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE pending_offers SET file_sha256 = ?1, chunk_hashes = ?2 WHERE transfer_id = ?3",
                rusqlite::params![file_sha256, chunk_hashes, transfer_id],
            )?;
            Ok(())
        })
    }

    pub fn get_pending_offers_for_user(&self, user_id: &str) -> Result<Vec<PendingOfferRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT transfer_id, from_user_id, to_user_id, filename, file_size,
                        file_sha256, chunk_hashes, file_server_url, folder_id, status
                 FROM pending_offers
                 WHERE to_user_id = ?1 AND status IN ('pending', 'accepted')
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([user_id], |row| {
                Ok(PendingOfferRow {
                    transfer_id: row.get(0)?,
                    from_user_id: row.get(1)?,
                    to_user_id: row.get(2)?,
                    filename: row.get(3)?,
                    file_size: row.get(4)?,
                    file_sha256: row.get(5)?,
                    chunk_hashes: row.get(6)?,
                    file_server_url: row.get(7)?,
                    folder_id: row.get(8)?,
                    status: row.get(9)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn insert_pending_folder_offer(
        &self,
        folder_id: &str,
        from_user_id: &str,
        to_user_id: &str,
        folder_name: &str,
        total_size: i64,
        file_count: i64,
        manifest: &str,
        file_server_url: Option<&str>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO pending_folder_offers
                 (folder_id, from_user_id, to_user_id, folder_name, total_size, file_count, manifest, file_server_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![folder_id, from_user_id, to_user_id, folder_name, total_size, file_count, manifest, file_server_url],
            )?;
            Ok(())
        })
    }

    pub fn update_pending_folder_offer_status(&self, folder_id: &str, status: &str) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE pending_folder_offers SET status = ?1 WHERE folder_id = ?2",
                rusqlite::params![status, folder_id],
            )?;
            Ok(())
        })
    }

    pub fn get_pending_folder_offers_for_user(&self, user_id: &str) -> Result<Vec<PendingFolderOfferRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT folder_id, from_user_id, to_user_id, folder_name, total_size,
                        file_count, manifest, file_server_url, status
                 FROM pending_folder_offers
                 WHERE to_user_id = ?1 AND status IN ('pending', 'accepted')
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([user_id], |row| {
                Ok(PendingFolderOfferRow {
                    folder_id: row.get(0)?,
                    from_user_id: row.get(1)?,
                    to_user_id: row.get(2)?,
                    folder_name: row.get(3)?,
                    total_size: row.get(4)?,
                    file_count: row.get(5)?,
                    manifest: row.get(6)?,
                    file_server_url: row.get(7)?,
                    status: row.get(8)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    /// Batch-fetch reactions for a set of message IDs.
    pub fn get_reactions_for_messages(&self, message_ids: &[String]) -> Result<Vec<ReactionRow>> {
        if message_ids.is_empty() {
            return Ok(vec![]);
        }

        self.with_conn(|conn| {
            let placeholders: Vec<String> = (1..=message_ids.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT id, message_id, user_id, emoji, created_at FROM reactions WHERE message_id IN ({})",
                placeholders.join(", ")
            );

            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> = message_ids
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();

            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok(ReactionRow {
                        id: row.get(0)?,
                        message_id: row.get(1)?,
                        user_id: row.get(2)?,
                        emoji: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(rows)
        })
    }
}

fn query_user_by_username(conn: &Connection, username: &str) -> Result<Option<UserRow>> {
    let mut stmt =
        conn.prepare("SELECT id, username, password, created_at FROM users WHERE username = ?1")?;

    let row = stmt
        .query_row([username], |row| {
            Ok(UserRow {
                id: row.get(0)?,
                username: row.get(1)?,
                password: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .optional()?;

    Ok(row)
}

/// #11: Cursor-based pagination — when `before` is provided, add `WHERE created_at < ?` clause.
fn query_messages(conn: &Connection, channel_id: &str, limit: u32, before: Option<&str>) -> Result<Vec<MessageRow>> {
    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match before {
        Some(before_ts) => (
            "SELECT m.id, m.channel_id, m.author_id, u.username, m.ciphertext, m.nonce, m.created_at
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             WHERE m.channel_id = ?1 AND m.created_at < ?3
             ORDER BY m.created_at DESC
             LIMIT ?2".to_string(),
            vec![
                Box::new(channel_id.to_string()) as Box<dyn rusqlite::types::ToSql>,
                Box::new(limit),
                Box::new(before_ts.to_string()),
            ],
        ),
        None => (
            "SELECT m.id, m.channel_id, m.author_id, u.username, m.ciphertext, m.nonce, m.created_at
             FROM messages m
             LEFT JOIN users u ON m.author_id = u.id
             WHERE m.channel_id = ?1
             ORDER BY m.created_at DESC
             LIMIT ?2".to_string(),
            vec![
                Box::new(channel_id.to_string()) as Box<dyn rusqlite::types::ToSql>,
                Box::new(limit),
            ],
        ),
    };

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                author_id: row.get(2)?,
                author_username: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "unknown".to_string()),
                ciphertext: row.get(4)?,
                nonce: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Extension trait for optional query results
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for std::result::Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
