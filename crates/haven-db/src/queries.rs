use crate::models::{MessageRow, ReactionRow, UserRow};
use crate::Database;
use anyhow::{Result, anyhow};
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

    pub fn get_user_by_id(&self, id: &str) -> Result<Option<UserRow>> {
        self.with_conn(|conn| query_user_by_id(conn, id))
    }

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

    pub fn get_messages(&self, channel_id: &str, limit: u32) -> Result<Vec<MessageRow>> {
        self.with_conn(|conn| query_messages(conn, channel_id, limit))
    }

    pub fn get_username_by_id(&self, id: &str) -> Result<String> {
        self.with_conn(|conn| {
            conn.query_row("SELECT username FROM users WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(|_| anyhow!("User not found: {}", id))
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

fn query_user_by_id(conn: &Connection, id: &str) -> Result<Option<UserRow>> {
    let mut stmt =
        conn.prepare("SELECT id, username, password, created_at FROM users WHERE id = ?1")?;

    let row = stmt
        .query_row([id], |row| {
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

fn query_messages(conn: &Connection, channel_id: &str, limit: u32) -> Result<Vec<MessageRow>> {
    // JOIN users to fetch author_username in a single query (eliminates N+1)
    let mut stmt = conn.prepare(
        "SELECT m.id, m.channel_id, m.author_id, u.username, m.ciphertext, m.nonce, m.created_at
         FROM messages m
         LEFT JOIN users u ON m.author_id = u.id
         WHERE m.channel_id = ?1
         ORDER BY m.created_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![channel_id, limit], |row| {
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
