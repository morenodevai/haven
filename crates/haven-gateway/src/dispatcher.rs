use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::warn;
use uuid::Uuid;

use haven_types::events::GatewayEvent;

/// Pre-serialized broadcast message. The JSON is serialized once in `broadcast()`
/// so N connections don't each pay the serialization cost. The `channel_id` is
/// extracted before serialization so connections can still filter by subscription
/// without deserializing.
#[derive(Debug, Clone)]
pub struct BroadcastMessage {
    pub channel_id: Option<Uuid>,
    pub json: Arc<str>,
}

/// Voice channel participant state.
#[derive(Debug, Clone)]
pub struct VoiceParticipant {
    pub user_id: Uuid,
    pub username: String,
    pub session_id: String,
    pub self_mute: bool,
    pub self_deaf: bool,
}

/// Messages that can be sent to a specific user's connection.
#[derive(Debug, Clone)]
pub enum UserMessage {
    /// A gateway event that will be JSON-serialized before sending.
    Event(GatewayEvent),
    /// Raw binary data to be sent as a WebSocket binary frame (zero-copy relay).
    /// Uses `Bytes` for O(1) cloning across multiple connections.
    Binary(Bytes),
}

/// Per-user targeted channel buffer depth. If a client can't keep up with 512
/// queued events, it is too slow and messages will be dropped with a warning.
const USER_CHANNEL_CAPACITY: usize = 2048;

/// Manages all connected clients and broadcasts events.
#[derive(Clone)]
pub struct Dispatcher {
    inner: Arc<DispatcherInner>,
}

struct DispatcherInner {
    /// Broadcast channel for pre-serialized gateway events -- all connected
    /// clients receive all events. Channel-scoped filtering is applied at
    /// the connection level, not here.
    broadcast_tx: broadcast::Sender<BroadcastMessage>,

    /// Track online users: user_id -> username
    online_users: RwLock<HashMap<Uuid, String>>,

    /// Per-user targeted send channels: user_id -> [(conn_id, sender)]
    /// Multiple connections per user are supported (multi-device).
    /// Bounded to USER_CHANNEL_CAPACITY; overflows are dropped with a warning.
    user_channels: RwLock<HashMap<Uuid, Vec<(Uuid, mpsc::Sender<UserMessage>)>>>,

    /// Voice state: channel_id -> (user_id -> participant)
    voice_states: RwLock<HashMap<Uuid, HashMap<Uuid, VoiceParticipant>>>,

    /// Per-user channel subscriptions: user_id -> set of channel_ids.
    /// Only events for subscribed channels are forwarded to each client.
    channel_subscriptions: RwLock<HashMap<Uuid, HashSet<Uuid>>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(8192);
        Self {
            inner: Arc::new(DispatcherInner {
                broadcast_tx,
                online_users: RwLock::new(HashMap::new()),
                user_channels: RwLock::new(HashMap::new()),
                voice_states: RwLock::new(HashMap::new()),
                channel_subscriptions: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Subscribe to gateway events. Returns a broadcast receiver of pre-serialized messages.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastMessage> {
        self.inner.broadcast_tx.subscribe()
    }

    /// Broadcast an event to all connected clients. The event is serialized once
    /// here; connections receive the pre-serialized JSON via `Arc<str>`.
    pub fn broadcast(&self, event: GatewayEvent) {
        let channel_id = event.channel_id();
        let json: Arc<str> = serde_json::to_string(&event)
            .expect("GatewayEvent serialization must not fail")
            .into();
        let msg = BroadcastMessage { channel_id, json };
        let _ = self.inner.broadcast_tx.send(msg);
    }

    /// Register a per-user targeted channel. Returns (conn_id, receiver).
    /// Multiple connections per user are supported for multi-device login.
    /// The channel is bounded to USER_CHANNEL_CAPACITY.
    pub async fn register_user_channel(&self, user_id: Uuid) -> (Uuid, mpsc::Receiver<UserMessage>) {
        let conn_id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(USER_CHANNEL_CAPACITY);
        self.inner.user_channels.write().await
            .entry(user_id)
            .or_default()
            .push((conn_id, tx));
        (conn_id, rx)
    }

    /// Unregister a specific connection for a user.
    pub async fn unregister_user_channel(&self, user_id: Uuid, conn_id: Uuid) {
        let mut channels = self.inner.user_channels.write().await;
        if let Some(conns) = channels.get_mut(&user_id) {
            conns.retain(|(cid, _)| *cid != conn_id);
            if conns.is_empty() {
                channels.remove(&user_id);
            }
        }
    }

    /// Send a targeted event to a specific user (all their devices).
    /// Uses `try_send` -- if a connection's buffer is full, the message is
    /// dropped with a warning (the client is too slow to keep up).
    pub async fn send_to_user(&self, user_id: Uuid, event: GatewayEvent) {
        let channels = self.inner.user_channels.read().await;
        if let Some(conns) = channels.get(&user_id) {
            for (conn_id, tx) in conns {
                match tx.try_send(UserMessage::Event(event.clone())) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            "Dropping targeted event for user {} conn {}: channel full ({} capacity). Client too slow.",
                            user_id, conn_id, USER_CHANNEL_CAPACITY
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Connection is gone; will be cleaned up on disconnect.
                    }
                }
            }
        }
    }

    /// Send raw binary data to a specific user (all their devices).
    /// Used for zero-copy relay of binary WebSocket frames (e.g., file chunks).
    /// Uses `Bytes` for O(1) cloning when the user has multiple connections.
    /// Uses `try_send` -- drops data for slow clients with a warning.
    pub async fn send_binary_to_user(&self, user_id: Uuid, data: Bytes) {
        let channels = self.inner.user_channels.read().await;
        if let Some(conns) = channels.get(&user_id) {
            for (conn_id, tx) in conns {
                match tx.try_send(UserMessage::Binary(data.clone())) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            "Dropping binary data for user {} conn {}: channel full ({} capacity)",
                            user_id, conn_id, USER_CHANNEL_CAPACITY
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
        }
    }

    /// Update a user's channel subscriptions. Replaces the entire set.
    pub async fn subscribe_channels(&self, user_id: Uuid, channel_ids: Vec<Uuid>) {
        let mut subs = self.inner.channel_subscriptions.write().await;
        subs.insert(user_id, channel_ids.into_iter().collect());
    }

    /// Remove all subscriptions for a user (called on disconnect).
    async fn clear_subscriptions(&self, user_id: Uuid) {
        let mut subs = self.inner.channel_subscriptions.write().await;
        subs.remove(&user_id);
    }

    /// Register a user as online.
    pub async fn user_online(&self, user_id: Uuid, username: String) {
        self.inner
            .online_users
            .write()
            .await
            .insert(user_id, username.clone());

        self.broadcast(GatewayEvent::PresenceUpdate {
            user_id,
            username,
            online: true,
        });
    }

    /// Register a user as offline. Removes this connection and only does
    /// full cleanup (leave voice, broadcast offline) when no connections remain.
    pub async fn user_offline(&self, user_id: Uuid, conn_id: Uuid) {
        // Remove this specific connection from the vec
        let remaining = {
            let mut channels = self.inner.user_channels.write().await;
            if let Some(conns) = channels.get_mut(&user_id) {
                conns.retain(|(cid, _)| *cid != conn_id);
                if conns.is_empty() {
                    channels.remove(&user_id);
                    0
                } else {
                    conns.len()
                }
            } else {
                0
            }
        };

        if remaining > 0 {
            // Other devices still connected -- don't mark offline
            return;
        }

        // All connections gone -- full cleanup
        let username = self
            .inner
            .online_users
            .write()
            .await
            .remove(&user_id)
            .unwrap_or_default();

        // Auto-leave voice on disconnect
        if let Some(channel_id) = self.voice_leave(user_id).await {
            self.broadcast(GatewayEvent::VoiceStateUpdate {
                channel_id,
                user_id,
                username: username.clone(),
                session_id: None,
                self_mute: false,
                self_deaf: false,
            });
        }

        self.clear_subscriptions(user_id).await;

        self.broadcast(GatewayEvent::PresenceUpdate {
            user_id,
            username,
            online: false,
        });
    }

    /// Get list of online users.
    pub async fn online_users(&self) -> Vec<(Uuid, String)> {
        self.inner
            .online_users
            .read()
            .await
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect()
    }

    /// Join a voice channel. Returns the list of existing participants.
    pub async fn voice_join(
        &self,
        channel_id: Uuid,
        user_id: Uuid,
        username: String,
        session_id: String,
    ) -> Vec<VoiceParticipant> {
        let mut voice_states = self.inner.voice_states.write().await;

        // Remove from any existing channel first, then clean up empty entries (L3)
        let mut emptied = Vec::new();
        for (&cid, participants) in voice_states.iter_mut() {
            if participants.remove(&user_id).is_some() && participants.is_empty() {
                emptied.push(cid);
            }
        }
        for cid in emptied {
            voice_states.remove(&cid);
        }

        let channel = voice_states.entry(channel_id).or_default();
        let existing: Vec<VoiceParticipant> = channel.values().cloned().collect();

        channel.insert(
            user_id,
            VoiceParticipant {
                user_id,
                username,
                session_id,
                self_mute: false,
                self_deaf: false,
            },
        );

        existing
    }

    /// Leave voice. Returns the channel_id they were in, if any.
    /// L3: Also removes the channel entry if no participants remain.
    pub async fn voice_leave(&self, user_id: Uuid) -> Option<Uuid> {
        let mut voice_states = self.inner.voice_states.write().await;

        let mut left_channel = None;
        for (&channel_id, participants) in voice_states.iter_mut() {
            if participants.remove(&user_id).is_some() {
                left_channel = Some(channel_id);
                break;
            }
        }

        // Clean up empty channel entry
        if let Some(channel_id) = left_channel {
            if voice_states.get(&channel_id).map_or(false, |p| p.is_empty()) {
                voice_states.remove(&channel_id);
            }
        }

        left_channel
    }

    /// Relay voice audio data from sender to all other participants in the same channel.
    /// Uses `try_send` -- drops data for slow clients with a warning.
    pub async fn relay_voice_data(&self, sender_id: Uuid, data: String) {
        let voice_states = self.inner.voice_states.read().await;
        let channels = self.inner.user_channels.read().await;

        for (_channel_id, participants) in voice_states.iter() {
            if participants.contains_key(&sender_id) {
                let event = GatewayEvent::VoiceAudioData {
                    from_user_id: sender_id,
                    data,
                };
                for (&uid, _) in participants.iter() {
                    if uid != sender_id {
                        if let Some(conns) = channels.get(&uid) {
                            for (conn_id, tx) in conns {
                                match tx.try_send(UserMessage::Event(event.clone())) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        warn!(
                                            "Dropping voice data for user {} conn {}: channel full",
                                            uid, conn_id
                                        );
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                        }
                    }
                }
                return;
            }
        }
    }

    /// #10: Relay binary voice audio data to all other participants in the same channel.
    /// The frame is already built as [0x04][sender_uid(16)][payload] — just forward
    /// as a binary WebSocket frame. No base64 encoding, no JSON serialization.
    pub async fn relay_voice_data_binary(&self, sender_id: Uuid, data: Bytes) {
        let voice_states = self.inner.voice_states.read().await;
        let channels = self.inner.user_channels.read().await;

        for (_channel_id, participants) in voice_states.iter() {
            if participants.contains_key(&sender_id) {
                for (&uid, _) in participants.iter() {
                    if uid != sender_id {
                        if let Some(conns) = channels.get(&uid) {
                            for (conn_id, tx) in conns {
                                match tx.try_send(UserMessage::Binary(data.clone())) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        warn!(
                                            "Dropping binary voice data for user {} conn {}: channel full",
                                            uid, conn_id
                                        );
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                        }
                    }
                }
                return;
            }
        }
    }

    /// Update mute/deaf state. Returns (channel_id, updated participant) if in voice.
    pub async fn voice_update_state(
        &self,
        user_id: Uuid,
        self_mute: bool,
        self_deaf: bool,
    ) -> Option<(Uuid, VoiceParticipant)> {
        let mut voice_states = self.inner.voice_states.write().await;

        for (&channel_id, participants) in voice_states.iter_mut() {
            if let Some(p) = participants.get_mut(&user_id) {
                p.self_mute = self_mute;
                p.self_deaf = self_deaf;
                return Some((channel_id, p.clone()));
            }
        }

        None
    }
}
