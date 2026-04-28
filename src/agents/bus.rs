//! Shared message bus used by every agent.
//!
//! Implemented over `tokio::sync::broadcast`: every agent subscribes,
//! filters for the events it cares about, and publishes its own.
//! Channel capacity defaults to 4096 — slow consumers will simply drop
//! older messages rather than back-pressure the producer (acceptable for
//! a real-time scalper where the freshest tick is the only one that
//! matters).

use crate::agents::messages::AgentEvent;
use tokio::sync::broadcast::{self, Receiver, Sender};
use tracing::warn;

#[derive(Clone)]
pub struct MessageBus {
    tx: Sender<AgentEvent>,
}

impl MessageBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> Receiver<AgentEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. Errors are logged but never propagated — losing
    /// a subscriber must not break the producer.
    pub fn publish(&self, ev: AgentEvent) {
        if let Err(e) = self.tx.send(ev) {
            // No subscribers is fine (initial boot); log only when we
            // expected someone to care.
            warn!(err = %e, "event dropped — no subscribers?");
        }
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new(4096)
    }
}
