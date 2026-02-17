use std::collections::HashMap;
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
    /// Broadcast channel for gateway events — all connected clients receive all events
    broadcast_tx: broadcast::Sender<GatewayEvent>,

    /// Track online users: user_id -> username
    online_users: RwLock<HashMap<Uuid, String>>,

    /// Per-user targeted send channels: user_id -> (conn_id, sender)
    user_channels: RwLock<HashMap<Uuid, (Uuid, mpsc::UnboundedSender<GatewayEvent>)>>,

    /// Voice state: channel_id -> (user_id -> participant)
    voice_states: RwLock<HashMap<Uuid, HashMap<Uuid, VoiceParticipant>>>,
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
    pub async fn register_user_channel(&self, user_id: Uuid) -> (Uuid, mpsc::UnboundedReceiver<GatewayEvent>) {
        let conn_id = Uuid::new_v4();
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.user_channels.write().await.insert(user_id, (conn_id, tx));
        (conn_id, rx)
    }

    /// Unregister a per-user targeted channel, but only if conn_id matches.
    pub async fn unregister_user_channel(&self, user_id: Uuid, conn_id: Uuid) {
        let mut channels = self.inner.user_channels.write().await;
        if let Some((stored_conn_id, _)) = channels.get(&user_id) {
            if *stored_conn_id == conn_id {
                channels.remove(&user_id);
            }
        }
    }

    /// Send a targeted event to a specific user.
    pub async fn send_to_user(&self, user_id: Uuid, event: GatewayEvent) {
        let channels = self.inner.user_channels.read().await;
        if let Some((_, tx)) = channels.get(&user_id) {
            let _ = tx.send(event);
        }
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

    /// Register a user as offline. Only cleans up if conn_id matches.
    pub async fn user_offline(&self, user_id: Uuid, conn_id: Uuid) {
        // Only clean up if this connection still owns the user channel
        let is_current = {
            let channels = self.inner.user_channels.read().await;
            channels.get(&user_id).map_or(false, |(cid, _)| *cid == conn_id)
        };

        if !is_current {
            // A newer connection has taken over — don't touch anything
            return;
        }

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

        self.unregister_user_channel(user_id, conn_id).await;

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

        // Remove from any existing channel first
        for (_, participants) in voice_states.iter_mut() {
            participants.remove(&user_id);
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
    pub async fn voice_leave(&self, user_id: Uuid) -> Option<Uuid> {
        let mut voice_states = self.inner.voice_states.write().await;

        for (&channel_id, participants) in voice_states.iter_mut() {
            if participants.remove(&user_id).is_some() {
                return Some(channel_id);
            }
        }

        None
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
                        if let Some((_, tx)) = channels.get(&uid) {
                            let _ = tx.send(event.clone());
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
