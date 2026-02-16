use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS users (
            id          TEXT PRIMARY KEY,
            username    TEXT NOT NULL UNIQUE,
            password    TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS channels (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS messages (
            id              TEXT PRIMARY KEY,
            channel_id      TEXT NOT NULL REFERENCES channels(id),
            author_id       TEXT NOT NULL REFERENCES users(id),
            ciphertext      BLOB NOT NULL,
            nonce           BLOB NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_messages_channel
            ON messages(channel_id, created_at);

        CREATE TABLE IF NOT EXISTS reactions (
            id          TEXT PRIMARY KEY,
            message_id  TEXT NOT NULL REFERENCES messages(id),
            user_id     TEXT NOT NULL REFERENCES users(id),
            emoji       TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(message_id, user_id, emoji)
        );

        CREATE INDEX IF NOT EXISTS idx_reactions_message
            ON reactions(message_id);

        -- Seed the default general channel
        INSERT OR IGNORE INTO channels (id, name)
            VALUES ('00000000-0000-0000-0000-000000000001', 'general');
        ",
    )?;

    info!("Database migrations complete");
    Ok(())
}
