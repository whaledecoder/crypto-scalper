//! Watchdog — listens for `Heartbeat` events from every agent and
//! freezes the bot if any agent goes silent for too long.
//!
//! Why: a hung WebSocket loop or a stuck LLM call could leave the
//! system staring at stale data while positions drift. The watchdog
//! protects against silent failure by triggering a `Freeze` (and a
//! Telegram alert via `MonitorAgent`) when liveness drops.

use crate::agents::messages::{AgentEvent, AgentId, ControlCommand};
use crate::agents::MessageBus;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    /// Agents we expect to publish heartbeats. Set to empty to disable.
    pub watched: Vec<AgentId>,
    /// Maximum seconds without a heartbeat before the watchdog triggers.
    pub liveness_timeout_secs: i64,
    /// How often to evaluate liveness (seconds).
    pub check_interval_secs: u64,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            watched: vec![
                AgentId::Data,
                AgentId::Feeds,
                AgentId::Survival,
                AgentId::Learning,
            ],
            liveness_timeout_secs: 90,
            check_interval_secs: 15,
        }
    }
}

pub fn spawn(bus: MessageBus, cfg: WatchdogConfig) -> JoinHandle<()> {
    let last_seen: Arc<Mutex<HashMap<AgentId, DateTime<Utc>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let frozen: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let last_seen_evt = last_seen.clone();
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!("watchdog agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Heartbeat { from, ts } => {
                    last_seen_evt.lock().insert(from, ts);
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    });

    // Periodic check task.
    let bus_check = bus.clone();
    let last_seen_check = last_seen.clone();
    let frozen_check = frozen.clone();
    tokio::spawn(async move {
        // Seed all watched agents with the current time so we don't
        // immediately fire on startup.
        {
            let now = Utc::now();
            let mut g = last_seen_check.lock();
            for a in &cfg.watched {
                g.insert(*a, now);
            }
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(cfg.check_interval_secs)).await;
            let now = Utc::now();
            let stale: Vec<(AgentId, i64)> = {
                let g = last_seen_check.lock();
                cfg.watched
                    .iter()
                    .filter_map(|a| {
                        g.get(a)
                            .map(|t| (*a, (now - *t).num_seconds()))
                            .filter(|(_, age)| *age > cfg.liveness_timeout_secs)
                    })
                    .collect()
            };
            if !stale.is_empty() {
                let mut g = frozen_check.lock();
                if !*g {
                    *g = true;
                    let summary = stale
                        .iter()
                        .map(|(a, age)| format!("{}({}s)", a.as_str(), age))
                        .collect::<Vec<_>>()
                        .join(", ");
                    warn!(stale = %summary, "watchdog: agent liveness lost — freezing");
                    bus_check.publish(AgentEvent::ControlCommand(ControlCommand::Freeze {
                        reason: format!("watchdog: stale agents {summary}"),
                    }));
                }
            } else if *frozen_check.lock() {
                *frozen_check.lock() = false;
                bus_check.publish(AgentEvent::ControlCommand(ControlCommand::Unfreeze));
                info!("watchdog: liveness restored — unfreezing");
            }
        }
    })
}
