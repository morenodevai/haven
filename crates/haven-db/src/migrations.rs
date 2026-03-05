use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

/// Current schema version. Increment this and add a new migration function
/// to the `MIGRATIONS` array when the schema changes.
const CURRENT_VERSION: u32 = 4;

/// Each migration is a function that takes a connection and applies changes.
/// Migrations are applied sequentially starting from the current version + 1.
type MigrationFn = fn(&Connection) -> Result<()>;

/// Ordered list of migrations. Index 0 = version 1, index 1 = version 2, etc.
const MIGRATIONS: &[MigrationFn] = &[
    migrate_v1,
    migrate_v2,
    migrate_v3,
    migrate_v4,
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

    // #25: Each migration + version bump is wrapped in a transaction.
    // BEGIN IMMEDIATE acquires a write lock immediately, preventing concurrent
    // writers from interleaving. On error, the entire migration is rolled back.
    for version in (current + 1)..=CURRENT_VERSION {
        let idx = (version - 1) as usize;
        info!("Applying migration v{}", version);

        conn.execute_batch("BEGIN IMMEDIATE")?;
        match MIGRATIONS[idx](conn) {
            Ok(()) => {
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [version],
                )?;
                conn.execute_batch("COMMIT")?;
                info!("Migration v{} applied successfully", version);
            }
            Err(e) => {
                conn.execute_batch("ROLLBACK").ok();
                return Err(anyhow::anyhow!("Migration v{} failed: {}", version, e));
            }
        }
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

/// Version 2: File upload support — files table for metadata,
/// encrypted blobs stored on disk in ./uploads/{id}.
fn migrate_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS files (
            id          TEXT PRIMARY KEY,
            uploader_id TEXT NOT NULL REFERENCES users(id),
            filename    TEXT NOT NULL,
            size        INTEGER NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )?;
    Ok(())
}

/// Version 3: Seed the file-sharing channel for P2P file transfers.
fn migrate_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        INSERT OR IGNORE INTO channels (id, name)
            VALUES ('00000000-0000-0000-0000-000000000003', 'file-sharing');
        ",
    )?;
    Ok(())
}

/// Version 4: Pending offers for transfer resume — persists file/folder offers
/// so reconnecting clients can recover missed offers.
fn migrate_v4(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS pending_offers (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            transfer_id     TEXT NOT NULL,
            from_user_id    TEXT NOT NULL,
            to_user_id      TEXT NOT NULL,
            filename        TEXT NOT NULL,
            file_size       INTEGER NOT NULL,
            file_sha256     TEXT,
            chunk_hashes    TEXT,
            file_server_url TEXT,
            folder_id       TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            status          TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE INDEX IF NOT EXISTS idx_pending_offers_to_user
            ON pending_offers(to_user_id, status);

        CREATE UNIQUE INDEX IF NOT EXISTS idx_pending_offers_transfer
            ON pending_offers(transfer_id);

        CREATE TABLE IF NOT EXISTS pending_folder_offers (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            folder_id       TEXT NOT NULL UNIQUE,
            from_user_id    TEXT NOT NULL,
            to_user_id      TEXT NOT NULL,
            folder_name     TEXT NOT NULL,
            total_size      INTEGER NOT NULL,
            file_count      INTEGER NOT NULL,
            manifest        TEXT NOT NULL,
            file_server_url TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            status          TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE INDEX IF NOT EXISTS idx_pending_folder_offers_to_user
            ON pending_folder_offers(to_user_id, status);
        ",
    )?;
    Ok(())
}
