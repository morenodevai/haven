//! UDP relay for the Haven Transfer Protocol (HTP).
//!
//! A single UDP socket that authenticates clients via JWT, then relays
//! encrypted file transfer packets between them. This is the data plane
//! for high-speed file transfers — control messages (OFFER/ACCEPT/NACK/DONE)
//! still travel over the WebSocket on port 3210.
//!
//! ## Authentication
//!
//! Clients authenticate with their first UDP packet:
//!   [0x00][jwt_len(2 bytes BE)][jwt_bytes...]
//!
//! Server responds:
//!   [0x00][0x01] (OK) or [0x00][0x00] (reject)
//!
//! After authentication, the client's SocketAddr is mapped to their user_id.
//! All subsequent packets from that address are treated as authenticated.
//!
//! ## Data Packets
//!
//! After auth, all packets use the HTP wire format:
//!   [0x48][0x54][session_id(4)][sequence(8)][flags(2)][encrypted_payload...]
//!
//! The relay routes by session_id: each session has exactly two participants
//! (sender and receiver). Packets from one are forwarded to the other.
//!
//! ## Session Lifecycle
//!
//! Sessions are created implicitly when the first HTP packet arrives for a
//! session_id from an authenticated user. The second user to send a packet
//! with the same session_id joins as the other side. Packets are then relayed
//! bidirectionally.
//!
//! Sessions are cleaned up after 60 seconds of inactivity.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use jsonwebtoken::{decode, DecodingKey, Validation};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use haven_types::api::Claims;

/// HTP magic bytes.
const HTP_MAGIC: [u8; 2] = [0x48, 0x54];

/// HTP header size.
const HTP_HEADER_SIZE: usize = 16;

/// Auth handshake marker byte.
const AUTH_MARKER: u8 = 0x00;

/// Maximum JWT size in auth handshake.
const MAX_JWT_SIZE: usize = 8192;

/// Maximum UDP packet size we'll accept.
const MAX_PACKET_SIZE: usize = 2048;

/// Session inactivity timeout.
const SESSION_TIMEOUT_SECS: u64 = 60;

/// Socket receive buffer size (4 MB).
const RECV_BUF_SIZE: usize = 4 * 1024 * 1024;

/// Socket send buffer size (4 MB).
const SEND_BUF_SIZE: usize = 4 * 1024 * 1024;

/// An authenticated client.
struct AuthenticatedClient {
    user_id: Uuid,
    username: String,
    last_seen: Instant,
}

/// A relay session — two endpoints exchanging HTP packets.
struct RelaySession {
    /// The first user to send a packet for this session.
    side_a: (Uuid, SocketAddr),
    /// The second user to join (None until they send their first packet).
    side_b: Option<(Uuid, SocketAddr)>,
    last_activity: Instant,
}

/// Shared state for the UDP relay.
pub struct UdpRelayState {
    inner: Arc<UdpRelayInner>,
}

struct UdpRelayInner {
    jwt_secret: String,
    /// Maps SocketAddr → authenticated client info.
    clients: RwLock<HashMap<SocketAddr, AuthenticatedClient>>,
    /// Maps session_id (u32) → relay session (two endpoints).
    sessions: RwLock<HashMap<u32, RelaySession>>,
}

impl UdpRelayState {
    pub fn new(jwt_secret: String) -> Self {
        Self {
            inner: Arc::new(UdpRelayInner {
                jwt_secret,
                clients: RwLock::new(HashMap::new()),
                sessions: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Run the UDP relay. This is the main loop — runs until the task is cancelled.
    pub async fn run(self, socket: Arc<UdpSocket>) {
        // Set socket buffer sizes
        let sock_ref = socket2::SockRef::from(&*socket);
        if let Err(e) = sock_ref.set_recv_buffer_size(RECV_BUF_SIZE) {
            warn!("UDP relay: failed to set recv buffer: {}", e);
        }
        if let Err(e) = sock_ref.set_send_buffer_size(SEND_BUF_SIZE) {
            warn!("UDP relay: failed to set send buffer: {}", e);
        }

        // Spawn cleanup task
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                Self::cleanup_stale(&inner).await;
            }
        });

        let mut buf = vec![0u8; MAX_PACKET_SIZE];

        info!("UDP relay: listening for HTP packets");

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, src)) => {
                    if len == 0 {
                        continue;
                    }
                    self.handle_packet(&socket, &buf[..len], src).await;
                }
                Err(e) => {
                    // On Windows, ICMP port unreachable causes recv to fail with
                    // ConnectionReset. This is normal — just continue.
                    if e.kind() == std::io::ErrorKind::ConnectionReset {
                        continue;
                    }
                    error!("UDP relay: recv error: {}", e);
                }
            }
        }
    }

    /// Handle a single incoming UDP packet.
    async fn handle_packet(&self, socket: &UdpSocket, data: &[u8], src: SocketAddr) {
        // Check if this is an auth handshake (first byte = 0x00)
        if data[0] == AUTH_MARKER {
            self.handle_auth(socket, data, src).await;
            return;
        }

        // Check if this is an HTP data packet (magic = "HT")
        if data.len() >= HTP_HEADER_SIZE && data[0..2] == HTP_MAGIC {
            self.handle_htp_packet(socket, data, src).await;
            return;
        }

        warn!(
            "UDP relay: unknown packet type 0x{:02x} from {}",
            data[0], src
        );
    }

    /// Handle JWT authentication handshake.
    async fn handle_auth(&self, socket: &UdpSocket, data: &[u8], src: SocketAddr) {
        // Format: [0x00][jwt_len(2 BE)][jwt_bytes...]
        if data.len() < 3 {
            let _ = socket.send_to(&[0x00, 0x00], src).await;
            return;
        }

        let jwt_len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if jwt_len == 0 || jwt_len > MAX_JWT_SIZE || data.len() < 3 + jwt_len {
            let _ = socket.send_to(&[0x00, 0x00], src).await;
            return;
        }

        let jwt_bytes = &data[3..3 + jwt_len];
        let jwt_str = match std::str::from_utf8(jwt_bytes) {
            Ok(s) => s,
            Err(_) => {
                let _ = socket.send_to(&[0x00, 0x00], src).await;
                return;
            }
        };

        let token_data = match decode::<Claims>(
            jwt_str,
            &DecodingKey::from_secret(self.inner.jwt_secret.as_bytes()),
            &Validation::default(),
        ) {
            Ok(data) => data,
            Err(_) => {
                let _ = socket.send_to(&[0x00, 0x00], src).await;
                return;
            }
        };

        let user_id = token_data.claims.sub;
        let username = token_data.claims.username.clone();
        info!("UDP relay: authenticated {} ({}) from {}", username, user_id, src);

        // Register client
        {
            let mut clients = self.inner.clients.write().await;
            clients.insert(
                src,
                AuthenticatedClient {
                    user_id,
                    username,
                    last_seen: Instant::now(),
                },
            );
        }

        // Send OK
        let _ = socket.send_to(&[0x00, 0x01], src).await;
    }

    /// Handle an HTP data packet — route to the other side of the session.
    async fn handle_htp_packet(&self, socket: &UdpSocket, data: &[u8], src: SocketAddr) {
        // Verify sender is authenticated
        let sender_id = {
            let mut clients = self.inner.clients.write().await;
            match clients.get_mut(&src) {
                Some(client) => {
                    client.last_seen = Instant::now();
                    client.user_id
                }
                None => {
                    debug!("UDP relay: unauthenticated packet from {}", src);
                    return;
                }
            }
        };

        // Parse session_id from HTP header (bytes 2..6)
        let session_id = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);

        // Look up or create the session
        let dest_addr = {
            let mut sessions = self.inner.sessions.write().await;
            let session = sessions.entry(session_id).or_insert_with(|| {
                debug!(
                    "UDP relay: new session {} from user {} ({})",
                    session_id, sender_id, src
                );
                RelaySession {
                    side_a: (sender_id, src),
                    side_b: None,
                    last_activity: Instant::now(),
                }
            });

            session.last_activity = Instant::now();

            // Determine the destination
            if session.side_a.0 == sender_id {
                // Update address in case it changed (NAT rebinding)
                session.side_a.1 = src;
                session.side_b.as_ref().map(|b| b.1)
            } else if session.side_b.as_ref().is_some_and(|b| b.0 == sender_id) {
                // Update address
                session.side_b.as_mut().unwrap().1 = src;
                Some(session.side_a.1)
            } else if session.side_b.is_none() {
                // Second participant joining
                debug!(
                    "UDP relay: user {} ({}) joined session {}",
                    sender_id, src, session_id
                );
                session.side_b = Some((sender_id, src));
                Some(session.side_a.1)
            } else {
                // Third party trying to join — reject
                warn!(
                    "UDP relay: user {} rejected from session {} (full)",
                    sender_id, session_id
                );
                None
            }
        };

        // Forward the packet as-is (already encrypted by the client)
        if let Some(dest) = dest_addr {
            if let Err(e) = socket.send_to(data, dest).await {
                warn!("UDP relay: send error to {}: {}", dest, e);
            }
        }
        // If dest is None, the other side hasn't connected yet — packet is dropped.
        // The sender's congestion control will handle this (it measures RTT, not loss).
    }

    /// Remove stale clients and sessions.
    async fn cleanup_stale(inner: &UdpRelayInner) {
        let now = Instant::now();
        let timeout = std::time::Duration::from_secs(SESSION_TIMEOUT_SECS);

        // Clean up stale sessions
        {
            let mut sessions = inner.sessions.write().await;
            let before = sessions.len();
            sessions.retain(|id, session| {
                let alive = now.duration_since(session.last_activity) < timeout;
                if !alive {
                    debug!("UDP relay: cleaning up stale session {}", id);
                }
                alive
            });
            let removed = before - sessions.len();
            if removed > 0 {
                info!("UDP relay: cleaned up {} stale sessions", removed);
            }
        }

        // Clean up stale clients (2x session timeout)
        {
            let client_timeout = std::time::Duration::from_secs(SESSION_TIMEOUT_SECS * 2);
            let mut clients = inner.clients.write().await;
            let before = clients.len();
            clients.retain(|addr, client| {
                let alive = now.duration_since(client.last_seen) < client_timeout;
                if !alive {
                    debug!(
                        "UDP relay: removing stale client {} ({})",
                        client.user_id, addr
                    );
                }
                alive
            });
            let removed = before - clients.len();
            if removed > 0 {
                info!("UDP relay: cleaned up {} stale clients", removed);
            }
        }
    }
}
