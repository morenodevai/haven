use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;

use haven_types::events::GatewayEvent;

/// Voice channel participant state.
#[derive(Debug, Clone)]
pub struct VoiceParticipant {
    pub user_id: Uuid,
    pub username: String,
    pub session_id: String,
    pub self_mute: bool,
    pub self_deaf: bool,
}

/// Manages all connected clients and broadcasts events.
#[derive(Clone)]
pub struct Dispatcher {
    inner: Arc<DispatcherInner>,
}

struct DispatcherInner {
    /// Broadcast channel for gateway events — all connected clients receive all events.
    /// Channel-scoped filtering is applied at the connection level, not here.
    broadcast_tx: broadcast::Sender<GatewayEvent>,

    /// Track online users: user_id -> username
    online_users: RwLock<HashMap<Uuid, String>>,

    /// Per-user targeted send channels: user_id -> [(conn_id, sender)]
    /// Multiple connections per user are supported (multi-device).
    user_channels: RwLock<HashMap<Uuid, Vec<(Uuid, mpsc::UnboundedSender<GatewayEvent>)>>>,

    /// Voice state: channel_id -> (user_id -> participant)
    voice_states: RwLock<HashMap<Uuid, HashMap<Uuid, VoiceParticipant>>>,

    /// Per-user channel subscriptions: user_id -> set of channel_ids.
    /// Only events for subscribed channels are forwarded to each client.
    channel_subscriptions: RwLock<HashMap<Uuid, HashSet<Uuid>>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
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

    /// Subscribe to gateway events. Returns a broadcast receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<GatewayEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    /// Broadcast an event to all connected clients.
    pub fn broadcast(&self, event: GatewayEvent) {
        let _ = self.inner.broadcast_tx.send(event);
    }

    /// Register a per-user targeted channel. Returns (conn_id, receiver).
    /// Multiple connections per user are supported for multi-device login.
    pub async fn register_user_channel(&self, user_id: Uuid) -> (Uuid, mpsc::UnboundedReceiver<GatewayEvent>) {
        let conn_id = Uuid::new_v4();
        let (tx, rx) = mpsc::unbounded_channel();
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
    pub async fn send_to_user(&self, user_id: Uuid, event: GatewayEvent) {
        let channels = self.inner.user_channels.read().await;
        if let Some(conns) = channels.get(&user_id) {
            for (_, tx) in conns {
                let _ = tx.send(event.clone());
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
            // Other devices still connected — don't mark offline
            return;
        }

        // All connections gone — full cleanup
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
                            for (_, tx) in conns {
                                let _ = tx.send(event.clone());
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
