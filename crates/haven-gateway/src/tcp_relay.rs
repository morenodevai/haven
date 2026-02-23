//! TCP relay for native file transfers.
//!
//! A bare TCP listener that authenticates clients via JWT, then relays binary
//! frames between connected users. This bypasses the WebSocket + browser
//! bottleneck to saturate gigabit connections.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use jsonwebtoken::{decode, DecodingKey, Validation};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

use haven_types::api::Claims;

/// Capacity of per-user outbound channel.
const USER_CHANNEL_CAPACITY: usize = 256;

/// 4 MB socket buffers for throughput.
const SOCKET_BUF_SIZE: usize = 4 * 1024 * 1024;

/// Maximum frame size (16 MB — 1MB chunk + overhead is well within this).
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Maximum JWT token size in the auth handshake.
const MAX_JWT_SIZE: usize = 8192;

// Frame types
const FRAME_FILE_CHUNK: u8 = 0x10;
const FRAME_FILE_ACK: u8 = 0x11;
const FRAME_FILE_DONE: u8 = 0x12;
const FRAME_FILE_CANCEL: u8 = 0x13;

/// Shared state for all TCP relay connections.
#[derive(Clone)]
pub struct TcpRelayState {
    inner: Arc<TcpRelayInner>,
}

struct TcpRelayInner {
    connections: RwLock<HashMap<Uuid, mpsc::Sender<Bytes>>>,
    jwt_secret: String,
}

impl TcpRelayState {
    pub fn new(jwt_secret: String) -> Self {
        Self {
            inner: Arc::new(TcpRelayInner {
                connections: RwLock::new(HashMap::new()),
                jwt_secret,
            }),
        }
    }

    /// Start the TCP relay listener. Runs until the task is cancelled.
    pub async fn run(self, listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("TCP relay: new connection from {}", addr);
                    let state = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = state.handle_connection(stream).await {
                            warn!("TCP relay connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("TCP relay accept error: {}", e);
                }
            }
        }
    }

    async fn handle_connection(
        &self,
        stream: tokio::net::TcpStream,
    ) -> anyhow::Result<()> {
        // Set socket options for throughput
        let sock_ref = socket2::SockRef::from(&stream);
        sock_ref.set_nodelay(true)?;
        sock_ref.set_send_buffer_size(SOCKET_BUF_SIZE)?;
        sock_ref.set_recv_buffer_size(SOCKET_BUF_SIZE)?;

        let (mut reader, mut writer) = stream.into_split();

        // --- Auth handshake ---
        // Client sends: [0x00][jwt_len(2 bytes BE)][jwt_bytes]
        // Server responds: [0x00][0x01] (OK) or [0x00][0x00] (reject)

        let marker = reader.read_u8().await?;
        if marker != 0x00 {
            writer.write_all(&[0x00, 0x00]).await?;
            return Ok(());
        }

        let jwt_len = reader.read_u16().await? as usize;
        if jwt_len == 0 || jwt_len > MAX_JWT_SIZE {
            writer.write_all(&[0x00, 0x00]).await?;
            return Ok(());
        }

        let mut jwt_buf = vec![0u8; jwt_len];
        reader.read_exact(&mut jwt_buf).await?;

        let jwt_str = std::str::from_utf8(&jwt_buf)?;
        let token_data = match decode::<Claims>(
            jwt_str,
            &DecodingKey::from_secret(self.inner.jwt_secret.as_bytes()),
            &Validation::default(),
        ) {
            Ok(data) => data,
            Err(_) => {
                writer.write_all(&[0x00, 0x00]).await?;
                return Ok(());
            }
        };

        let user_id = token_data.claims.sub;
        info!(
            "TCP relay: authenticated {} ({})",
            token_data.claims.username, user_id
        );

        // Send OK
        writer.write_all(&[0x00, 0x01]).await?;

        // --- Register user ---
        let (tx, mut rx) = mpsc::channel::<Bytes>(USER_CHANNEL_CAPACITY);

        {
            let mut conns = self.inner.connections.write().await;
            conns.insert(user_id, tx);
        }

        // Spawn writer task: drains rx and writes length-prefixed frames to TCP
        let write_handle = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let len = frame.len() as u32;
                if writer.write_all(&len.to_be_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(&frame).await.is_err() {
                    break;
                }
            }
        });

        // --- Read loop ---
        let result = self.read_loop(&mut reader, user_id).await;

        // --- Cleanup ---
        write_handle.abort();
        {
            let mut conns = self.inner.connections.write().await;
            conns.remove(&user_id);
        }
        info!("TCP relay: user {} disconnected", user_id);

        result
    }

    /// Read length-prefixed frames from the client and relay to target users.
    ///
    /// Binary frame protocol (after auth):
    ///   [frame_len(4 bytes BE)][type(1)][target_uid(16)][transfer_id(16)][chunk_idx(4)][payload...]
    ///
    /// Types:
    ///   0x10 FileChunk  — min 37 bytes header + payload
    ///   0x11 FileAck    — exactly 37 bytes
    ///   0x12 FileDone   — exactly 33 bytes
    ///   0x13 FileCancel — exactly 33 bytes
    ///
    /// The relay swaps target_uid with sender_uid and forwards to the target.
    async fn read_loop(
        &self,
        reader: &mut tokio::net::tcp::OwnedReadHalf,
        sender_id: Uuid,
    ) -> anyhow::Result<()> {
        loop {
            // Read frame length (4 bytes BE)
            let frame_len = match reader.read_u32().await {
                Ok(len) => len as usize,
                Err(_) => return Ok(()), // clean disconnect
            };

            if frame_len == 0 || frame_len > MAX_FRAME_SIZE {
                warn!(
                    "TCP relay: invalid frame length {} from {}",
                    frame_len, sender_id
                );
                return Ok(());
            }

            // Read frame body
            let mut frame = vec![0u8; frame_len];
            reader.read_exact(&mut frame).await?;

            if frame.is_empty() {
                continue;
            }

            let msg_type = frame[0];

            match msg_type {
                // FileChunk (0x10) / FileAck (0x11): need >= 37 bytes (1+16+16+4)
                FRAME_FILE_CHUNK | FRAME_FILE_ACK => {
                    if frame.len() < 37 {
                        warn!("TCP relay: frame too short for type 0x{:02x}", msg_type);
                        continue;
                    }
                    let target_id =
                        Uuid::from_bytes(frame[1..17].try_into().unwrap());

                    // Swap target_uid with sender_uid (zero-copy relay)
                    frame[1..17].copy_from_slice(sender_id.as_bytes());

                    self.send_to_user(target_id, Bytes::from(frame)).await;
                }
                // FileDone (0x12) / FileCancel (0x13): need >= 33 bytes (1+16+16)
                FRAME_FILE_DONE | FRAME_FILE_CANCEL => {
                    if frame.len() < 33 {
                        warn!("TCP relay: frame too short for type 0x{:02x}", msg_type);
                        continue;
                    }
                    let target_id =
                        Uuid::from_bytes(frame[1..17].try_into().unwrap());

                    frame[1..17].copy_from_slice(sender_id.as_bytes());

                    if msg_type == FRAME_FILE_DONE {
                        let transfer_id =
                            Uuid::from_bytes(frame[17..33].try_into().unwrap());
                        info!(
                            "TCP relay: DONE {} -> {} (transfer {})",
                            sender_id, target_id, transfer_id
                        );
                    }

                    self.send_to_user(target_id, Bytes::from(frame)).await;
                }
                _ => {
                    warn!(
                        "TCP relay: unknown frame type 0x{:02x} from {}",
                        msg_type, sender_id
                    );
                }
            }
        }
    }

    /// Send a frame to a connected user. Drops the frame if the channel is full.
    async fn send_to_user(&self, user_id: Uuid, data: Bytes) {
        let conns = self.inner.connections.read().await;
        if let Some(tx) = conns.get(&user_id) {
            if let Err(e) = tx.try_send(data) {
                match e {
                    mpsc::error::TrySendError::Full(_) => {
                        warn!(
                            "TCP relay: channel full for user {}, dropping frame",
                            user_id
                        );
                    }
                    mpsc::error::TrySendError::Closed(_) => {}
                }
            }
        }
    }
}
