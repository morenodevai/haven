use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tracing::{debug, error, info};

// ── STUN/TURN constants ─────────────────────────────────────────────

const STUN_HEADER_SIZE: usize = 20;
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;

// Message types
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;
const ALLOCATE_REQUEST: u16 = 0x0003;
const ALLOCATE_RESPONSE: u16 = 0x0103;
const ALLOCATE_ERROR: u16 = 0x0113;
const REFRESH_REQUEST: u16 = 0x0004;
const REFRESH_RESPONSE: u16 = 0x0104;
const CREATE_PERMISSION_REQUEST: u16 = 0x0008;
const CREATE_PERMISSION_RESPONSE: u16 = 0x0108;
const CHANNEL_BIND_REQUEST: u16 = 0x0009;
const CHANNEL_BIND_RESPONSE: u16 = 0x0109;
const SEND_INDICATION: u16 = 0x0016;
const DATA_INDICATION: u16 = 0x0017;

// Attribute types
const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_CHANNEL_NUMBER: u16 = 0x000C;
const ATTR_LIFETIME: u16 = 0x000D;
const ATTR_DATA: u16 = 0x0013;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_REQUESTED_TRANSPORT: u16 = 0x0019;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_SOFTWARE: u16 = 0x8022;

const DEFAULT_LIFETIME: u32 = 600; // 10 minutes
const MAX_LIFETIME: u32 = 3600; // 1 hour
const CHANNEL_DATA_HEADER_SIZE: usize = 4;
const SOFTWARE_NAME: &[u8] = b"Haven TURN";

// ── Configuration ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TurnConfig {
    pub udp_port: u16,
    pub public_ip: IpAddr,
    pub realm: String,
    pub username: String,
    pub password: String,
}

// ── Allocation state ─────────────────────────────────────────────────

struct Allocation {
    relay_socket: Arc<UdpSocket>,
    #[allow(dead_code)]
    relay_addr: SocketAddr,
    permissions: HashMap<IpAddr, Instant>,
    channels: HashMap<u16, SocketAddr>,    // channel_number -> peer addr
    channel_rev: HashMap<SocketAddr, u16>, // peer addr -> channel_number
    expires: Instant,
}

// ── TURN Server ──────────────────────────────────────────────────────

pub struct TurnServer {
    config: TurnConfig,
    allocations: Arc<RwLock<HashMap<SocketAddr, Allocation>>>,
    hmac_key: Vec<u8>,
}

impl TurnServer {
    pub fn new(config: TurnConfig) -> Self {
        let hmac_key = compute_long_term_key(&config.username, &config.realm, &config.password);
        Self {
            config,
            allocations: Arc::new(RwLock::new(HashMap::new())),
            hmac_key,
        }
    }

    /// Run the UDP TURN listener.
    pub async fn run_udp(&self, port: u16) -> anyhow::Result<()> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let socket = Arc::new(UdpSocket::bind(addr).await?);
        info!("TURN relay listening on UDP {}", addr);

        // Spawn reaper for expired allocations
        let allocs = self.allocations.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let expired: Vec<SocketAddr> = {
                    let map = allocs.read().unwrap();
                    let now = Instant::now();
                    map.iter()
                        .filter(|(_, a)| now >= a.expires)
                        .map(|(k, _)| *k)
                        .collect()
                };
                if !expired.is_empty() {
                    let mut map = allocs.write().unwrap();
                    for addr in &expired {
                        info!("TURN: reaping expired allocation for {}", addr);
                        map.remove(addr);
                    }
                }
            }
        });

        let mut buf = vec![0u8; 65536];
        loop {
            let (n, src) = socket.recv_from(&mut buf).await?;
            if n < 1 {
                continue;
            }

            // Check if this is ChannelData (first two bits are not 00 for STUN)
            // ChannelData: channel numbers 0x4000-0x7FFF → first byte 0x40-0x7F
            if buf[0] >= 0x40 && buf[0] <= 0x7F && n >= CHANNEL_DATA_HEADER_SIZE {
                self.handle_channel_data(&socket, src, &buf[..n]).await;
            } else if n >= STUN_HEADER_SIZE {
                self.handle_stun_message(&socket, src, &buf[..n]).await;
            }
        }
    }

    /// Handle a STUN/TURN message.
    async fn handle_stun_message(&self, socket: &Arc<UdpSocket>, src: SocketAddr, data: &[u8]) {
        if data.len() < STUN_HEADER_SIZE {
            return;
        }

        let msg_type = u16::from_be_bytes([data[0], data[1]]);
        let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
        let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

        if cookie != STUN_MAGIC_COOKIE {
            return;
        }

        if data.len() < STUN_HEADER_SIZE + msg_len {
            return;
        }

        let txn_id: [u8; 12] = data[8..20].try_into().unwrap();
        let attrs = parse_attributes(&data[STUN_HEADER_SIZE..STUN_HEADER_SIZE + msg_len]);

        match msg_type {
            BINDING_REQUEST => {
                self.handle_binding(socket, src, &txn_id).await;
            }
            ALLOCATE_REQUEST => {
                self.handle_allocate(socket, src, &txn_id, &attrs, data).await;
            }
            REFRESH_REQUEST => {
                self.handle_refresh(socket, src, &txn_id, &attrs, data).await;
            }
            CREATE_PERMISSION_REQUEST => {
                self.handle_create_permission(socket, src, &txn_id, &attrs, data).await;
            }
            CHANNEL_BIND_REQUEST => {
                self.handle_channel_bind(socket, src, &txn_id, &attrs, data).await;
            }
            SEND_INDICATION => {
                self.handle_send_indication(src, &attrs).await;
            }
            _ => {
                debug!("TURN: unknown message type 0x{:04x} from {}", msg_type, src);
            }
        }
    }

    /// STUN Binding: respond with XOR-MAPPED-ADDRESS.
    async fn handle_binding(&self, socket: &Arc<UdpSocket>, src: SocketAddr, txn_id: &[u8; 12]) {
        let mut attrs = Vec::new();
        append_xor_mapped_address(&mut attrs, ATTR_XOR_MAPPED_ADDRESS, src, txn_id);
        append_software(&mut attrs);

        let resp = build_stun_message(BINDING_RESPONSE, txn_id, &attrs);
        let _ = socket.send_to(&resp, src).await;
    }

    /// TURN Allocate: either 401 challenge or create allocation.
    async fn handle_allocate(
        &self,
        socket: &Arc<UdpSocket>,
        src: SocketAddr,
        txn_id: &[u8; 12],
        attrs: &HashMap<u16, Vec<u8>>,
        raw: &[u8],
    ) {
        // Check if already allocated
        let already = self.allocations.read().unwrap().contains_key(&src);
        if already {
            let resp = build_error_response(ALLOCATE_ERROR, txn_id, 437, "Allocation Mismatch");
            let _ = socket.send_to(&resp, src).await;
            return;
        }

        // If no MESSAGE-INTEGRITY, send 401 challenge
        if !attrs.contains_key(&ATTR_MESSAGE_INTEGRITY) {
            let nonce = generate_nonce();
            let mut resp_attrs = Vec::new();
            append_error_code(&mut resp_attrs, 401, "Unauthorized");
            append_string_attr(&mut resp_attrs, ATTR_REALM, &self.config.realm);
            append_string_attr(&mut resp_attrs, ATTR_NONCE, &nonce);

            let resp = build_stun_message(ALLOCATE_ERROR, txn_id, &resp_attrs);
            let _ = socket.send_to(&resp, src).await;
            return;
        }

        // Verify credentials
        if !self.verify_message_integrity(raw, attrs) {
            let resp = build_error_response(ALLOCATE_ERROR, txn_id, 401, "Unauthorized");
            let _ = socket.send_to(&resp, src).await;
            return;
        }

        // Check REQUESTED-TRANSPORT (must be UDP = 17)
        if let Some(transport) = attrs.get(&ATTR_REQUESTED_TRANSPORT) {
            if transport.len() >= 4 && transport[0] != 17 {
                let resp = build_error_response(ALLOCATE_ERROR, txn_id, 442, "Unsupported Transport Protocol");
                let _ = socket.send_to(&resp, src).await;
                return;
            }
        }

        // Create relay socket
        let relay_socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                error!("TURN: failed to create relay socket: {}", e);
                let resp = build_error_response(ALLOCATE_ERROR, txn_id, 508, "Insufficient Capacity");
                let _ = socket.send_to(&resp, src).await;
                return;
            }
        };

        let relay_local = relay_socket.local_addr().unwrap();
        let relay_addr = SocketAddr::new(self.config.public_ip, relay_local.port());

        let lifetime = extract_lifetime(attrs).unwrap_or(DEFAULT_LIFETIME).min(MAX_LIFETIME);

        info!(
            "TURN: allocation created for {} -> relay {} (lifetime {}s)",
            src, relay_addr, lifetime
        );

        // Spawn relay task: relay_socket -> client via main socket
        // CRITICAL: never hold lock across await — look up, drop, then send
        let relay_rx = relay_socket.clone();
        let main_socket = socket.clone();
        let client_addr = src;
        let allocs_clone = self.allocations.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, peer_addr) = match relay_rx.recv_from(&mut buf).await {
                    Ok(r) => r,
                    Err(_) => break,
                };

                // Look up permission + channel binding, then DROP lock before I/O
                let action = {
                    let allocs = allocs_clone.read().unwrap();
                    let alloc = match allocs.get(&client_addr) {
                        Some(a) => a,
                        None => break,
                    };
                    if !alloc.permissions.contains_key(&peer_addr.ip()) {
                        continue;
                    }
                    alloc.channel_rev.get(&peer_addr).copied()
                }; // lock dropped here

                if let Some(channel) = action {
                    // Send as ChannelData (fast path — what WebRTC uses)
                    let padded_n = (n + 3) & !3;
                    let mut cd = vec![0u8; CHANNEL_DATA_HEADER_SIZE + padded_n];
                    cd[0..2].copy_from_slice(&channel.to_be_bytes());
                    cd[2..4].copy_from_slice(&(n as u16).to_be_bytes());
                    cd[4..4 + n].copy_from_slice(&buf[..n]);
                    let _ = main_socket.send_to(&cd[..CHANNEL_DATA_HEADER_SIZE + padded_n], client_addr).await;
                } else {
                    // Send as Data indication
                    let txn_id = [0u8; 12];
                    let mut attrs_buf = Vec::new();
                    append_xor_peer_address(&mut attrs_buf, peer_addr, &txn_id);
                    append_data_attr(&mut attrs_buf, &buf[..n]);
                    let indication = build_stun_message(DATA_INDICATION, &txn_id, &attrs_buf);
                    let _ = main_socket.send_to(&indication, client_addr).await;
                }
            }
        });

        // Store allocation
        {
            let mut allocs = self.allocations.write().unwrap();
            allocs.insert(src, Allocation {
                relay_socket,
                relay_addr,
                permissions: HashMap::new(),
                channels: HashMap::new(),
                channel_rev: HashMap::new(),
                expires: Instant::now() + Duration::from_secs(lifetime as u64),
            });
        }

        // Build success response
        let mut resp_attrs = Vec::new();
        append_xor_relayed_address(&mut resp_attrs, relay_addr, txn_id);
        append_xor_mapped_address(&mut resp_attrs, ATTR_XOR_MAPPED_ADDRESS, src, txn_id);
        append_lifetime(&mut resp_attrs, lifetime);
        append_software(&mut resp_attrs);

        let resp = build_stun_message_with_integrity(ALLOCATE_RESPONSE, txn_id, &resp_attrs, &self.hmac_key);
        let _ = socket.send_to(&resp, src).await;
    }

    /// TURN Refresh: extend or delete allocation.
    async fn handle_refresh(
        &self,
        socket: &Arc<UdpSocket>,
        src: SocketAddr,
        txn_id: &[u8; 12],
        attrs: &HashMap<u16, Vec<u8>>,
        raw: &[u8],
    ) {
        if !self.verify_message_integrity(raw, attrs) {
            let resp = build_error_response(REFRESH_RESPONSE | 0x0010, txn_id, 401, "Unauthorized");
            let _ = socket.send_to(&resp, src).await;
            return;
        }

        let lifetime = extract_lifetime(attrs).unwrap_or(DEFAULT_LIFETIME).min(MAX_LIFETIME);

        {
            let mut allocs = self.allocations.write().unwrap();
            if let Some(alloc) = allocs.get_mut(&src) {
                if lifetime == 0 {
                    info!("TURN: allocation deleted for {} (lifetime=0)", src);
                    allocs.remove(&src);
                } else {
                    alloc.expires = Instant::now() + Duration::from_secs(lifetime as u64);
                    debug!("TURN: allocation refreshed for {} (lifetime {}s)", src, lifetime);
                }
            }
        }

        let mut resp_attrs = Vec::new();
        append_lifetime(&mut resp_attrs, lifetime);

        let resp = build_stun_message_with_integrity(REFRESH_RESPONSE, txn_id, &resp_attrs, &self.hmac_key);
        let _ = socket.send_to(&resp, src).await;
    }

    /// TURN CreatePermission: allow traffic from a peer IP.
    async fn handle_create_permission(
        &self,
        socket: &Arc<UdpSocket>,
        src: SocketAddr,
        txn_id: &[u8; 12],
        attrs: &HashMap<u16, Vec<u8>>,
        raw: &[u8],
    ) {
        if !self.verify_message_integrity(raw, attrs) {
            return;
        }

        {
            let mut allocs = self.allocations.write().unwrap();
            if let Some(alloc) = allocs.get_mut(&src) {
                if let Some(peer_data) = attrs.get(&ATTR_XOR_PEER_ADDRESS) {
                    if let Some(peer_addr) = decode_xor_address(peer_data, txn_id) {
                        alloc.permissions.insert(
                            peer_addr.ip(),
                            Instant::now() + Duration::from_secs(300),
                        );
                        debug!("TURN: permission created for {} -> peer {}", src, peer_addr.ip());
                    }
                }
            }
        }

        let resp = build_stun_message_with_integrity(CREATE_PERMISSION_RESPONSE, txn_id, &[], &self.hmac_key);
        let _ = socket.send_to(&resp, src).await;
    }

    /// TURN ChannelBind: map channel number to peer address.
    async fn handle_channel_bind(
        &self,
        socket: &Arc<UdpSocket>,
        src: SocketAddr,
        txn_id: &[u8; 12],
        attrs: &HashMap<u16, Vec<u8>>,
        raw: &[u8],
    ) {
        if !self.verify_message_integrity(raw, attrs) {
            return;
        }

        let channel_number = match attrs.get(&ATTR_CHANNEL_NUMBER) {
            Some(data) if data.len() >= 4 => u16::from_be_bytes([data[0], data[1]]),
            _ => return,
        };

        if !(0x4000..=0x7FFE).contains(&channel_number) {
            return;
        }

        let peer_addr = match attrs.get(&ATTR_XOR_PEER_ADDRESS) {
            Some(data) => match decode_xor_address(data, txn_id) {
                Some(addr) => addr,
                None => return,
            },
            None => return,
        };

        {
            let mut allocs = self.allocations.write().unwrap();
            if let Some(alloc) = allocs.get_mut(&src) {
                alloc.permissions.insert(
                    peer_addr.ip(),
                    Instant::now() + Duration::from_secs(300),
                );
                alloc.channels.insert(channel_number, peer_addr);
                alloc.channel_rev.insert(peer_addr, channel_number);
                debug!("TURN: channel 0x{:04x} bound to {} for client {}", channel_number, peer_addr, src);
            }
        }

        let resp = build_stun_message_with_integrity(CHANNEL_BIND_RESPONSE, txn_id, &[], &self.hmac_key);
        let _ = socket.send_to(&resp, src).await;
    }

    /// Handle Send indication: relay data to peer via relay socket.
    async fn handle_send_indication(
        &self,
        src: SocketAddr,
        attrs: &HashMap<u16, Vec<u8>>,
    ) {
        let txn_id = [0u8; 12];
        let peer_addr = match attrs.get(&ATTR_XOR_PEER_ADDRESS) {
            Some(data) => match decode_xor_address(data, &txn_id) {
                Some(addr) => addr,
                None => return,
            },
            None => return,
        };

        let payload = match attrs.get(&ATTR_DATA) {
            Some(data) => data,
            None => return,
        };

        // Look up relay socket, drop lock, then send
        let relay = {
            let allocs = self.allocations.read().unwrap();
            allocs.get(&src).and_then(|alloc| {
                if alloc.permissions.contains_key(&peer_addr.ip()) {
                    Some(alloc.relay_socket.clone())
                } else {
                    None
                }
            })
        };

        if let Some(relay_socket) = relay {
            let _ = relay_socket.send_to(payload, peer_addr).await;
        }
    }

    /// Handle ChannelData: relay via channel binding.
    /// CRITICAL HOT PATH — this is every video packet from the remote peer.
    async fn handle_channel_data(&self, _socket: &Arc<UdpSocket>, src: SocketAddr, data: &[u8]) {
        if data.len() < CHANNEL_DATA_HEADER_SIZE {
            return;
        }

        let channel = u16::from_be_bytes([data[0], data[1]]);
        let length = u16::from_be_bytes([data[2], data[3]]) as usize;

        if data.len() < CHANNEL_DATA_HEADER_SIZE + length {
            return;
        }

        let payload = &data[CHANNEL_DATA_HEADER_SIZE..CHANNEL_DATA_HEADER_SIZE + length];

        // Look up relay socket + peer addr, then DROP lock before I/O
        let target = {
            let allocs = self.allocations.read().unwrap();
            allocs.get(&src).and_then(|alloc| {
                alloc.channels.get(&channel).map(|&peer| (alloc.relay_socket.clone(), peer))
            })
        }; // lock dropped

        if let Some((relay_socket, peer_addr)) = target {
            let _ = relay_socket.send_to(payload, peer_addr).await;
        }
    }

    /// Verify MESSAGE-INTEGRITY on a STUN message.
    fn verify_message_integrity(&self, raw: &[u8], attrs: &HashMap<u16, Vec<u8>>) -> bool {
        // Check username matches
        if let Some(username_data) = attrs.get(&ATTR_USERNAME) {
            let username = String::from_utf8_lossy(username_data);
            if username != self.config.username {
                return false;
            }
        } else {
            return false;
        }

        let integrity = match attrs.get(&ATTR_MESSAGE_INTEGRITY) {
            Some(data) if data.len() == 20 => data,
            _ => return false,
        };

        // Find position of MESSAGE-INTEGRITY attribute in raw data
        // The HMAC covers everything up to (but not including) the MESSAGE-INTEGRITY attribute,
        // with the length field adjusted to include MESSAGE-INTEGRITY (24 bytes: 4 header + 20 value)
        let mi_pos = find_attr_position(raw, ATTR_MESSAGE_INTEGRITY);
        if mi_pos == 0 {
            return false;
        }

        // Build the data to HMAC: header (with adjusted length) + attributes before MI
        let mut hmac_input = Vec::from(&raw[..mi_pos]);
        // Adjust the STUN message length to include MESSAGE-INTEGRITY (up to end of MI attr)
        let adjusted_len = (mi_pos - STUN_HEADER_SIZE + 24) as u16;
        hmac_input[2] = (adjusted_len >> 8) as u8;
        hmac_input[3] = (adjusted_len & 0xFF) as u8;

        let expected = compute_hmac_sha1(&self.hmac_key, &hmac_input);
        expected == integrity.as_slice()
    }

    /// Run TURN-over-TCP for a single connection.
    /// RFC 6062: STUN messages are sent directly over TCP (no framing — STUN has its own length).
    /// ChannelData has its own 4-byte header.
    pub async fn handle_tcp_connection(
        self: &Arc<Self>,
        stream: tokio::net::TcpStream,
        addr: SocketAddr,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::sync::mpsc;

        info!("TURN TCP connection from {}", addr);
        let (mut reader, mut writer) = stream.into_split();

        // Channel for relay-back: relay socket → TCP writer (big buffer for video bursts)
        let (relay_tx, mut relay_rx) = mpsc::channel::<Vec<u8>>(1024);

        // Spawn writer task: drains relay_tx and STUN responses onto TCP
        let (stun_tx, mut stun_rx) = mpsc::channel::<Vec<u8>>(64);
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = stun_rx.recv() => {
                        match msg {
                            Some(data) => {
                                if writer.write_all(&data).await.is_err() { break; }
                            }
                            None => break,
                        }
                    }
                    msg = relay_rx.recv() => {
                        match msg {
                            Some(data) => {
                                if writer.write_all(&data).await.is_err() { break; }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        let mut buf = vec![0u8; 65536];

        loop {
            // Read first 2 bytes to determine message type
            let mut first2 = [0u8; 2];
            if reader.read_exact(&mut first2).await.is_err() {
                break;
            }

            // ChannelData: first byte 0x40-0x7F
            if first2[0] >= 0x40 && first2[0] <= 0x7F {
                let channel = u16::from_be_bytes(first2);
                let mut data_len_buf = [0u8; 2];
                if reader.read_exact(&mut data_len_buf).await.is_err() {
                    break;
                }
                let data_len = u16::from_be_bytes(data_len_buf) as usize;
                if data_len > buf.len() {
                    break;
                }
                // Read payload + padding to 4-byte boundary
                let padded = (data_len + 3) & !3;
                if reader.read_exact(&mut buf[..padded]).await.is_err() {
                    break;
                }

                // Look up, drop lock, then send (never hold lock across await)
                let target = {
                    let allocs = self.allocations.read().unwrap();
                    allocs.get(&addr).and_then(|alloc| {
                        alloc.channels.get(&channel).map(|&peer| (alloc.relay_socket.clone(), peer))
                    })
                };
                if let Some((relay_socket, peer_addr)) = target {
                    let _ = relay_socket.send_to(&buf[..data_len], peer_addr).await;
                }
                continue;
            }

            // STUN message — first2 is the message type
            let msg_type = u16::from_be_bytes(first2);
            let mut msg_len_buf = [0u8; 2];
            if reader.read_exact(&mut msg_len_buf).await.is_err() {
                break;
            }
            let msg_len = u16::from_be_bytes(msg_len_buf) as usize;

            // Read cookie + txn_id + attributes
            let remaining = 16 + msg_len; // 4 cookie + 12 txn_id + attrs
            if remaining > buf.len() {
                break;
            }
            if reader.read_exact(&mut buf[..remaining]).await.is_err() {
                break;
            }

            // Reconstruct full STUN message
            let mut full_msg = Vec::with_capacity(STUN_HEADER_SIZE + msg_len);
            full_msg.extend_from_slice(&first2);
            full_msg.extend_from_slice(&msg_len_buf);
            full_msg.extend_from_slice(&buf[..remaining]);

            let cookie = u32::from_be_bytes([full_msg[4], full_msg[5], full_msg[6], full_msg[7]]);
            if cookie != STUN_MAGIC_COOKIE {
                continue;
            }

            let txn_id: [u8; 12] = full_msg[8..20].try_into().unwrap();
            let attrs = parse_attributes(&full_msg[STUN_HEADER_SIZE..]);

            let response = match msg_type {
                BINDING_REQUEST => {
                    let mut resp_attrs = Vec::new();
                    append_xor_mapped_address(&mut resp_attrs, ATTR_XOR_MAPPED_ADDRESS, addr, &txn_id);
                    append_software(&mut resp_attrs);
                    Some(build_stun_message(BINDING_RESPONSE, &txn_id, &resp_attrs))
                }
                ALLOCATE_REQUEST => {
                    let resp = self.handle_allocate_tcp(addr, &txn_id, &attrs, &full_msg).await;
                    // After successful allocation, spawn relay-back task
                    {
                        let relay_socket = {
                            let allocs = self.allocations.read().unwrap();
                            allocs.get(&addr).map(|a| a.relay_socket.clone())
                        };
                        if let Some(relay_socket) = relay_socket {
                            let tx = relay_tx.clone();
                            let allocs_ref = self.allocations.clone();
                            let client_addr = addr;
                            tokio::spawn(async move {
                                let mut rbuf = vec![0u8; 65536];
                                loop {
                                    let (n, peer_addr) = match relay_socket.recv_from(&mut rbuf).await {
                                        Ok(r) => r,
                                        Err(_) => break,
                                    };

                                    // Look up permission + channel, drop lock, then build frame
                                    let action = {
                                        let allocs = allocs_ref.read().unwrap();
                                        let alloc = match allocs.get(&client_addr) {
                                            Some(a) => a,
                                            None => break,
                                        };
                                        if !alloc.permissions.contains_key(&peer_addr.ip()) {
                                            continue;
                                        }
                                        alloc.channel_rev.get(&peer_addr).copied()
                                    }; // lock dropped

                                    let frame = if let Some(channel) = action {
                                        let padded_n = (n + 3) & !3;
                                        let mut cd = vec![0u8; CHANNEL_DATA_HEADER_SIZE + padded_n];
                                        cd[0..2].copy_from_slice(&channel.to_be_bytes());
                                        cd[2..4].copy_from_slice(&(n as u16).to_be_bytes());
                                        cd[4..4 + n].copy_from_slice(&rbuf[..n]);
                                        cd
                                    } else {
                                        let txn = [0u8; 12];
                                        let mut ind_attrs = Vec::new();
                                        append_xor_peer_address(&mut ind_attrs, peer_addr, &txn);
                                        append_data_attr(&mut ind_attrs, &rbuf[..n]);
                                        build_stun_message(DATA_INDICATION, &txn, &ind_attrs)
                                    };

                                    if tx.send(frame).await.is_err() {
                                        break;
                                    }
                                }
                            });
                        }
                    }
                    Some(resp)
                }
                REFRESH_REQUEST => {
                    if !self.verify_message_integrity(&full_msg, &attrs) {
                        continue;
                    }
                    let lifetime = extract_lifetime(&attrs).unwrap_or(DEFAULT_LIFETIME).min(MAX_LIFETIME);
                    {
                        let mut allocs = self.allocations.write().unwrap();
                        if let Some(alloc) = allocs.get_mut(&addr) {
                            if lifetime == 0 {
                                allocs.remove(&addr);
                            } else {
                                alloc.expires = Instant::now() + Duration::from_secs(lifetime as u64);
                            }
                        }
                    }
                    let mut resp_attrs = Vec::new();
                    append_lifetime(&mut resp_attrs, lifetime);
                    Some(build_stun_message_with_integrity(REFRESH_RESPONSE, &txn_id, &resp_attrs, &self.hmac_key))
                }
                CREATE_PERMISSION_REQUEST => {
                    if !self.verify_message_integrity(&full_msg, &attrs) {
                        continue;
                    }
                    {
                        let mut allocs = self.allocations.write().unwrap();
                        if let Some(alloc) = allocs.get_mut(&addr) {
                            if let Some(peer_data) = attrs.get(&ATTR_XOR_PEER_ADDRESS) {
                                if let Some(peer_addr) = decode_xor_address(peer_data, &txn_id) {
                                    alloc.permissions.insert(peer_addr.ip(), Instant::now() + Duration::from_secs(300));
                                    debug!("TURN TCP: permission for {} -> {}", addr, peer_addr.ip());
                                }
                            }
                        }
                    }
                    Some(build_stun_message_with_integrity(CREATE_PERMISSION_RESPONSE, &txn_id, &[], &self.hmac_key))
                }
                CHANNEL_BIND_REQUEST => {
                    if !self.verify_message_integrity(&full_msg, &attrs) {
                        continue;
                    }
                    let channel_number = match attrs.get(&ATTR_CHANNEL_NUMBER) {
                        Some(data) if data.len() >= 4 => u16::from_be_bytes([data[0], data[1]]),
                        _ => continue,
                    };
                    if !(0x4000..=0x7FFE).contains(&channel_number) {
                        continue;
                    }
                    let peer_addr = match attrs.get(&ATTR_XOR_PEER_ADDRESS) {
                        Some(data) => match decode_xor_address(data, &txn_id) {
                            Some(a) => a,
                            None => continue,
                        },
                        None => continue,
                    };
                    {
                        let mut allocs = self.allocations.write().unwrap();
                        if let Some(alloc) = allocs.get_mut(&addr) {
                            alloc.permissions.insert(peer_addr.ip(), Instant::now() + Duration::from_secs(300));
                            alloc.channels.insert(channel_number, peer_addr);
                            alloc.channel_rev.insert(peer_addr, channel_number);
                            debug!("TURN TCP: channel 0x{:04x} -> {} for {}", channel_number, peer_addr, addr);
                        }
                    }
                    Some(build_stun_message_with_integrity(CHANNEL_BIND_RESPONSE, &txn_id, &[], &self.hmac_key))
                }
                SEND_INDICATION => {
                    let peer_addr = match attrs.get(&ATTR_XOR_PEER_ADDRESS) {
                        Some(data) => decode_xor_address(data, &txn_id),
                        None => None,
                    };
                    if let (Some(peer), Some(payload)) = (peer_addr, attrs.get(&ATTR_DATA)) {
                        let relay = {
                            let allocs = self.allocations.read().unwrap();
                            allocs.get(&addr).and_then(|alloc| {
                                if alloc.permissions.contains_key(&peer.ip()) {
                                    Some(alloc.relay_socket.clone())
                                } else {
                                    None
                                }
                            })
                        };
                        if let Some(relay_socket) = relay {
                            let _ = relay_socket.send_to(payload, peer).await;
                        }
                    }
                    None
                }
                _ => None,
            };

            if let Some(resp) = response {
                if stun_tx.send(resp).await.is_err() {
                    break;
                }
            }
        }

        // Clean up
        writer_task.abort();
        let mut allocs = self.allocations.write().unwrap();
        if allocs.remove(&addr).is_some() {
            info!("TURN: TCP allocation removed for {}", addr);
        }
    }

    /// Handle Allocate for TCP path — returns response bytes.
    async fn handle_allocate_tcp(
        &self,
        src: SocketAddr,
        txn_id: &[u8; 12],
        attrs: &HashMap<u16, Vec<u8>>,
        raw: &[u8],
    ) -> Vec<u8> {
        let already = self.allocations.read().unwrap().contains_key(&src);
        if already {
            return build_error_response(ALLOCATE_ERROR, txn_id, 437, "Allocation Mismatch");
        }

        if !attrs.contains_key(&ATTR_MESSAGE_INTEGRITY) {
            let nonce = generate_nonce();
            let mut resp_attrs = Vec::new();
            append_error_code(&mut resp_attrs, 401, "Unauthorized");
            append_string_attr(&mut resp_attrs, ATTR_REALM, &self.config.realm);
            append_string_attr(&mut resp_attrs, ATTR_NONCE, &nonce);
            return build_stun_message(ALLOCATE_ERROR, txn_id, &resp_attrs);
        }

        if !self.verify_message_integrity(raw, attrs) {
            return build_error_response(ALLOCATE_ERROR, txn_id, 401, "Unauthorized");
        }

        let relay_socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                error!("TURN TCP: failed to create relay socket: {}", e);
                return build_error_response(ALLOCATE_ERROR, txn_id, 508, "Insufficient Capacity");
            }
        };

        let relay_local = relay_socket.local_addr().unwrap();
        let relay_addr = SocketAddr::new(self.config.public_ip, relay_local.port());
        let lifetime = extract_lifetime(attrs).unwrap_or(DEFAULT_LIFETIME).min(MAX_LIFETIME);

        info!("TURN TCP: allocation created for {} -> relay {} (lifetime {}s)", src, relay_addr, lifetime);

        // Note: for TCP, relay-to-client data must go over the TCP stream.
        // The relay task for TCP is more complex — we store relay_socket but the
        // spawned relay reader task needs access to the TCP writer.
        // For simplicity, we store the relay socket and the main handle_tcp_connection
        // loop doesn't read from it. Instead we spawn a task that does.
        // TCP relay-back is handled by the caller checking for incoming relay data.

        {
            let mut allocs = self.allocations.write().unwrap();
            allocs.insert(src, Allocation {
                relay_socket,
                relay_addr,
                permissions: HashMap::new(),
                channels: HashMap::new(),
                channel_rev: HashMap::new(),
                expires: Instant::now() + Duration::from_secs(lifetime as u64),
            });
        }

        let mut resp_attrs = Vec::new();
        append_xor_relayed_address(&mut resp_attrs, relay_addr, txn_id);
        append_xor_mapped_address(&mut resp_attrs, ATTR_XOR_MAPPED_ADDRESS, src, txn_id);
        append_lifetime(&mut resp_attrs, lifetime);
        append_software(&mut resp_attrs);

        build_stun_message_with_integrity(ALLOCATE_RESPONSE, txn_id, &resp_attrs, &self.hmac_key)
    }

    /// Get TURN URLs for client ICE configuration.
    /// Both UDP and TCP use the same port (gateway port) — no extra port forwards needed.
    pub fn ice_urls(&self) -> Vec<String> {
        let ip = self.config.public_ip;
        let port = self.config.udp_port;
        vec![
            format!("turn:{}:{}?transport=udp", ip, port),
            format!("turn:{}:{}?transport=tcp", ip, port),
        ]
    }
}

// ── STUN message building helpers ────────────────────────────────────

fn build_stun_message(msg_type: u16, txn_id: &[u8; 12], attrs: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(STUN_HEADER_SIZE + attrs.len());
    msg.extend_from_slice(&msg_type.to_be_bytes());
    msg.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
    msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    msg.extend_from_slice(txn_id);
    msg.extend_from_slice(attrs);
    msg
}

fn build_stun_message_with_integrity(
    msg_type: u16,
    txn_id: &[u8; 12],
    attrs: &[u8],
    key: &[u8],
) -> Vec<u8> {
    // MESSAGE-INTEGRITY is 24 bytes (4 header + 20 HMAC-SHA1)
    let total_attr_len = attrs.len() + 24;

    let mut msg = Vec::with_capacity(STUN_HEADER_SIZE + total_attr_len);
    msg.extend_from_slice(&msg_type.to_be_bytes());
    msg.extend_from_slice(&(total_attr_len as u16).to_be_bytes());
    msg.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    msg.extend_from_slice(txn_id);
    msg.extend_from_slice(attrs);

    // Compute HMAC-SHA1 over everything so far
    let hmac = compute_hmac_sha1(key, &msg);

    // Append MESSAGE-INTEGRITY attribute
    msg.extend_from_slice(&ATTR_MESSAGE_INTEGRITY.to_be_bytes());
    msg.extend_from_slice(&20u16.to_be_bytes());
    msg.extend_from_slice(&hmac);

    msg
}

fn build_error_response(msg_type: u16, txn_id: &[u8; 12], code: u16, reason: &str) -> Vec<u8> {
    let mut attrs = Vec::new();
    append_error_code(&mut attrs, code, reason);
    build_stun_message(msg_type, txn_id, &attrs)
}

fn append_xor_mapped_address(buf: &mut Vec<u8>, attr_type: u16, addr: SocketAddr, txn_id: &[u8; 12]) {
    let xor_port = (addr.port()) ^ ((STUN_MAGIC_COOKIE >> 16) as u16);

    match addr.ip() {
        IpAddr::V4(ip) => {
            let ip_bytes = ip.octets();
            let cookie_bytes = STUN_MAGIC_COOKIE.to_be_bytes();
            let xor_ip = [
                ip_bytes[0] ^ cookie_bytes[0],
                ip_bytes[1] ^ cookie_bytes[1],
                ip_bytes[2] ^ cookie_bytes[2],
                ip_bytes[3] ^ cookie_bytes[3],
            ];

            buf.extend_from_slice(&attr_type.to_be_bytes());
            buf.extend_from_slice(&8u16.to_be_bytes()); // length
            buf.push(0); // reserved
            buf.push(0x01); // IPv4
            buf.extend_from_slice(&xor_port.to_be_bytes());
            buf.extend_from_slice(&xor_ip);
        }
        IpAddr::V6(ip) => {
            let ip_bytes = ip.octets();
            let mut xor_key = [0u8; 16];
            xor_key[..4].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
            xor_key[4..].copy_from_slice(txn_id);
            let mut xor_ip = [0u8; 16];
            for i in 0..16 {
                xor_ip[i] = ip_bytes[i] ^ xor_key[i];
            }

            buf.extend_from_slice(&attr_type.to_be_bytes());
            buf.extend_from_slice(&20u16.to_be_bytes()); // length
            buf.push(0); // reserved
            buf.push(0x02); // IPv6
            buf.extend_from_slice(&xor_port.to_be_bytes());
            buf.extend_from_slice(&xor_ip);
        }
    }
}

fn append_xor_relayed_address(buf: &mut Vec<u8>, addr: SocketAddr, txn_id: &[u8; 12]) {
    append_xor_mapped_address(buf, ATTR_XOR_RELAYED_ADDRESS, addr, txn_id);
}

fn append_xor_peer_address(buf: &mut Vec<u8>, addr: SocketAddr, txn_id: &[u8; 12]) {
    append_xor_mapped_address(buf, ATTR_XOR_PEER_ADDRESS, addr, txn_id);
}

fn append_error_code(buf: &mut Vec<u8>, code: u16, reason: &str) {
    let class = (code / 100) as u8;
    let number = (code % 100) as u8;
    let reason_bytes = reason.as_bytes();
    let value_len = 4 + reason_bytes.len();
    let padded_len = (value_len + 3) & !3;

    buf.extend_from_slice(&ATTR_ERROR_CODE.to_be_bytes());
    buf.extend_from_slice(&(padded_len as u16).to_be_bytes());
    buf.extend_from_slice(&[0, 0]); // reserved
    buf.push(class);
    buf.push(number);
    buf.extend_from_slice(reason_bytes);
    // Pad to 4-byte boundary
    for _ in 0..(padded_len - value_len) {
        buf.push(0);
    }
}

fn append_string_attr(buf: &mut Vec<u8>, attr_type: u16, value: &str) {
    let bytes = value.as_bytes();
    let padded_len = (bytes.len() + 3) & !3;

    buf.extend_from_slice(&attr_type.to_be_bytes());
    buf.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    buf.extend_from_slice(bytes);
    for _ in 0..(padded_len - bytes.len()) {
        buf.push(0);
    }
}

fn append_lifetime(buf: &mut Vec<u8>, lifetime: u32) {
    buf.extend_from_slice(&ATTR_LIFETIME.to_be_bytes());
    buf.extend_from_slice(&4u16.to_be_bytes());
    buf.extend_from_slice(&lifetime.to_be_bytes());
}

fn append_data_attr(buf: &mut Vec<u8>, data: &[u8]) {
    let padded_len = (data.len() + 3) & !3;
    buf.extend_from_slice(&ATTR_DATA.to_be_bytes());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    for _ in 0..(padded_len - data.len()) {
        buf.push(0);
    }
}

fn append_software(buf: &mut Vec<u8>) {
    append_string_attr(buf, ATTR_SOFTWARE, std::str::from_utf8(SOFTWARE_NAME).unwrap());
}

// ── STUN attribute parsing ───────────────────────────────────────────

fn parse_attributes(data: &[u8]) -> HashMap<u16, Vec<u8>> {
    let mut attrs = HashMap::new();
    let mut pos = 0;

    while pos + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let attr_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if pos + attr_len > data.len() {
            break;
        }

        attrs.insert(attr_type, data[pos..pos + attr_len].to_vec());

        // Advance past value + padding to 4-byte boundary
        pos += (attr_len + 3) & !3;
    }

    attrs
}

fn decode_xor_address(data: &[u8], txn_id: &[u8; 12]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }

    let family = data[1];
    let xor_port = u16::from_be_bytes([data[2], data[3]]);
    let port = xor_port ^ ((STUN_MAGIC_COOKIE >> 16) as u16);

    match family {
        0x01 => {
            // IPv4
            let cookie_bytes = STUN_MAGIC_COOKIE.to_be_bytes();
            let ip = Ipv4Addr::new(
                data[4] ^ cookie_bytes[0],
                data[5] ^ cookie_bytes[1],
                data[6] ^ cookie_bytes[2],
                data[7] ^ cookie_bytes[3],
            );
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        0x02 if data.len() >= 20 => {
            // IPv6
            let mut xor_key = [0u8; 16];
            xor_key[..4].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
            xor_key[4..].copy_from_slice(txn_id);
            let mut octets = [0u8; 16];
            for i in 0..16 {
                octets[i] = data[4 + i] ^ xor_key[i];
            }
            let ip = std::net::Ipv6Addr::from(octets);
            Some(SocketAddr::new(IpAddr::V6(ip), port))
        }
        _ => None,
    }
}

fn extract_lifetime(attrs: &HashMap<u16, Vec<u8>>) -> Option<u32> {
    attrs.get(&ATTR_LIFETIME).and_then(|data| {
        if data.len() >= 4 {
            Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
        } else {
            None
        }
    })
}

/// Find the byte position of an attribute in raw STUN message data.
fn find_attr_position(data: &[u8], target_type: u16) -> usize {
    let mut pos = STUN_HEADER_SIZE;
    while pos + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        if attr_type == target_type {
            return pos;
        }
        let attr_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4 + ((attr_len + 3) & !3);
    }
    0
}

// ── Crypto helpers ───────────────────────────────────────────────────

/// Compute long-term TURN credential key: MD5(username:realm:password)
fn compute_long_term_key(username: &str, realm: &str, password: &str) -> Vec<u8> {
    use md5::{Md5, Digest};
    let input = format!("{}:{}:{}", username, realm, password);
    let result = Md5::digest(input.as_bytes());
    result.to_vec()
}

/// Compute HMAC-SHA1
fn compute_hmac_sha1(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;
    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Generate a random nonce string.
fn generate_nonce() -> String {
    use std::time::SystemTime;
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", ts)
}
