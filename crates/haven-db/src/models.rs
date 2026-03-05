/// Database row types — these map directly to SQLite rows.
/// Distinct from haven-types API models to keep the DB layer independent.

pub struct UserRow {
    pub id: String,
    pub username: String,
    pub password: String,
    pub created_at: String,
}

pub struct MessageRow {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub author_username: String,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub created_at: String,
}

pub struct ReactionRow {
    pub id: String,
    pub message_id: String,
    pub user_id: String,
    pub emoji: String,
    pub created_at: String,
}

pub struct FileRow {
    pub id: String,
    pub uploader_id: String,
    pub filename: String,
    pub size: i64,
    pub created_at: String,
}

pub struct PendingOfferRow {
    pub transfer_id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    pub filename: String,
    pub file_size: i64,
    pub file_sha256: Option<String>,
    pub chunk_hashes: Option<String>,
    pub file_server_url: Option<String>,
    pub folder_id: Option<String>,
    pub status: String,
}

pub struct PendingFolderOfferRow {
    pub folder_id: String,
    pub from_user_id: String,
    pub to_user_id: String,
    pub folder_name: String,
    pub total_size: i64,
    pub file_count: i64,
    pub manifest: String,
    pub file_server_url: Option<String>,
    pub status: String,
}
