//! Shared message bus used by every agent.
//!
//! Implemented over `tokio::sync::broadcast`: every agent subscribes,
//! filters for the events it cares about, and publishes its own.
//! Channel capacity defaults to 4096 — slow consumers will simply drop
//! older messages rather than back-pressure the producer (acceptable for
//! a real-time scalper where the freshest tick is the only one that
//! matters).

use crate::agents::messages::AgentEvent;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast::{self, Receiver, Sender};
use tracing::warn;

#[derive(Clone)]
pub struct MessageBus {
    tx: Sender<AgentEvent>,
    last_warn_us: Arc<AtomicI64>,
}

impl MessageBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            tx,
            last_warn_us: Arc::new(AtomicI64::new(0)),
        }
    }

    pub fn subscribe(&self) -> Receiver<AgentEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. Errors are logged but never propagated — losing
    /// a subscriber must not break the producer.
    ///
    /// Throttle the "no subscribers" warning to at most once per second.
    /// This avoids flooding the log with thousands of identical lines
    /// during normal shutdown when every receiver drops at once and the
    /// data agent is still draining its tick buffer.
    pub fn publish(&self, ev: AgentEvent) {
        if let Err(e) = self.tx.send(ev) {
            let now_us = chrono::Utc::now().timestamp_micros();
            let last = self.last_warn_us.load(Ordering::Relaxed);
            if now_us.saturating_sub(last) > 1_000_000 {
                self.last_warn_us.store(now_us, Ordering::Relaxed);
                warn!(err = %e, "event dropped — no subscribers? (throttled, will not repeat for 1s)");
            }
        }
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new(4096)
    }
}
