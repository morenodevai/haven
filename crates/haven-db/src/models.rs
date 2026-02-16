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
