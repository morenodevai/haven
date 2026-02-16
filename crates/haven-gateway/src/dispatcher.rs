use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use haven_types::events::GatewayEvent;

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
}

impl Dispatcher {
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(DispatcherInner {
                broadcast_tx,
                online_users: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Subscribe to gateway events. Returns a broadcast receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<GatewayEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    /// Broadcast an event to all connected clients.
    pub fn broadcast(&self, event: GatewayEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.inner.broadcast_tx.send(event);
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

    /// Register a user as offline.
    pub async fn user_offline(&self, user_id: Uuid) {
        let username = self
            .inner
            .online_users
            .write()
            .await
            .remove(&user_id)
            .unwrap_or_default();

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
}
