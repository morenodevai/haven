use crate::models::{MessageRow, UserRow};
use crate::Database;
use anyhow::{Result, anyhow};
use rusqlite::Connection;

impl Database {
    // -- Users --

    pub fn create_user(&self, id: &str, username: &str, password_hash: &str) -> Result<()> {
        self.with_conn(|conn| {
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
        self.with_conn(|conn| {
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
    let mut stmt = conn.prepare(
        "SELECT id, channel_id, author_id, ciphertext, nonce, created_at
         FROM messages
         WHERE channel_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![channel_id, limit], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                author_id: row.get(2)?,
                ciphertext: row.get(3)?,
                nonce: row.get(4)?,
                created_at: row.get(5)?,
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
