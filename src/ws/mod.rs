use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::AppState;

// ---------------------------------------------------------------------------
// Event enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    NewListing {
        inscription_id: String,
        price_sats: u64,
        seller: String,
    },
    SaleConfirmed {
        inscription_id: String,
        price_sats: u64,
        buyer: String,
        tx_id: String,
    },
    OfferReceived {
        inscription_id: String,
        price_sats: u64,
        buyer: String,
    },
    PriceUpdate {
        inscription_id: String,
        old_price_sats: u64,
        new_price_sats: u64,
    },
}

// ---------------------------------------------------------------------------
// Subscription filter
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WsSubscribe {
    pub inscription_id: Option<String>,
    pub collection_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Broadcaster
// ---------------------------------------------------------------------------

pub struct WsBroadcaster {
    sender: broadcast::Sender<WsEvent>,
}

impl WsBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }

    /// Send an event to all connected subscribers.
    pub fn send(&self, event: WsEvent) {
        // A send error just means there are no active receivers; ignore it.
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.sender.subscribe()
    }
}

impl Default for WsBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper: does this event match the client's subscription filter?
// ---------------------------------------------------------------------------

fn event_matches(event: &WsEvent, filter: &Option<WsSubscribe>) -> bool {
    let filter = match filter {
        Some(f) => f,
        // No filter supplied — forward everything.
        None => return true,
    };

    // If neither field is set, forward everything.
    if filter.inscription_id.is_none() && filter.collection_id.is_none() {
        return true;
    }

    // Match on inscription_id when present in the filter.
    if let Some(ref wanted_id) = filter.inscription_id {
        let event_inscription_id: Option<&str> = match event {
            WsEvent::NewListing { inscription_id, .. } => Some(inscription_id),
            WsEvent::SaleConfirmed { inscription_id, .. } => Some(inscription_id),
            WsEvent::OfferReceived { inscription_id, .. } => Some(inscription_id),
            WsEvent::PriceUpdate { inscription_id, .. } => Some(inscription_id),
        };
        if let Some(eid) = event_inscription_id {
            if eid == wanted_id {
                return true;
            }
        }
    }

    // collection_id filtering: the current WsEvent variants don't carry a
    // collection_id, so we cannot match on it here. Future variants can add
    // collection_id fields and extend this logic.

    false
}

// ---------------------------------------------------------------------------
// Axum handler
// ---------------------------------------------------------------------------

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Step 1 – try to read an optional subscribe message within 5 seconds.
    let filter: Option<WsSubscribe> = tokio::time::timeout(
        Duration::from_secs(5),
        socket.recv(),
    )
    .await
    .ok()                          // timeout expired → None
    .and_then(|opt_msg| opt_msg)  // no message from client → None
    .and_then(|msg| match msg {
        Ok(Message::Text(text)) => {
            serde_json::from_str::<WsSubscribe>(&text).ok()
        }
        _ => None,
    });

    // Step 2 – subscribe to the broadcast channel.
    let mut rx = state.ws_broadcaster.subscribe();

    // Step 3 – forward matching events until the client disconnects.
    loop {
        match rx.recv().await {
            Ok(event) => {
                if !event_matches(&event, &filter) {
                    continue;
                }
                let json = match serde_json::to_string(&event) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if socket.send(Message::Text(json)).await.is_err() {
                    // Client disconnected.
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Receiver fell behind; skip missed messages and continue.
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Broadcaster shut down.
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(broadcaster: Arc<WsBroadcaster>) -> Router<AppState> {
    // We need to expose the broadcaster inside the handler via AppState, so
    // the Arc is stored on AppState. The argument here is kept for API
    // symmetry / future use (e.g. if this module owned its own sub-state).
    let _ = broadcaster; // already on AppState; suppress unused-variable lint
    Router::new().route("/ws", get(ws_handler))
}
