use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

/// Current schema version. Increment this and add a new migration function
/// to the `MIGRATIONS` array when the schema changes.
const CURRENT_VERSION: u32 = 1;

/// Each migration is a function that takes a connection and applies changes.
/// Migrations are applied sequentially starting from the current version + 1.
type MigrationFn = fn(&Connection) -> Result<()>;

/// Ordered list of migrations. Index 0 = version 1, index 1 = version 2, etc.
const MIGRATIONS: &[MigrationFn] = &[
    migrate_v1,
];

pub fn run(conn: &Connection) -> Result<()> {
    // Create the version tracking table if it doesn't exist.
    // This table always uses IF NOT EXISTS so it's safe on first run.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER NOT NULL,
            applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );"
    )?;

    let current = get_current_version(conn)?;
    info!("Database schema version: {} (latest: {})", current, CURRENT_VERSION);

    if current >= CURRENT_VERSION {
        return Ok(());
    }

    for version in (current + 1)..=CURRENT_VERSION {
        let idx = (version - 1) as usize;
        info!("Applying migration v{}", version);
        MIGRATIONS[idx](conn)?;

        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [version],
        )?;
        info!("Migration v{} applied successfully", version);
    }

    info!("Database migrations complete (now at v{})", CURRENT_VERSION);
    Ok(())
}

fn get_current_version(conn: &Connection) -> Result<u32> {
    let version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;
    Ok(version)
}

/// Version 1: Initial schema — all base tables.
fn migrate_v1(conn: &Connection) -> Result<()> {
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

        -- Seed the default voice channel
        INSERT OR IGNORE INTO channels (id, name)
            VALUES ('00000000-0000-0000-0000-000000000002', 'Voice');
        ",
    )?;
    Ok(())
}
