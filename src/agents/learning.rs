//! Learning agent — periodically rebuilds the `LearningPolicy` from the
//! trade journal and broadcasts the refresh event.

use crate::agents::messages::{AgentEvent, AgentId};
use crate::agents::MessageBus;
use crate::learning::{
    lessons::{LessonConfig, LessonExtractor},
    LearningPolicy, PerformanceMemory,
};
use crate::monitoring::TradeJournal;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub fn spawn(
    bus: MessageBus,
    journal: Arc<TradeJournal>,
    policy: LearningPolicy,
    cfg: LessonConfig,
    refresh_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!(refresh_secs, "learning agent starting");
        let extractor = LessonExtractor::new(cfg);
        let mut tick = tokio::time::interval(Duration::from_secs(refresh_secs.max(60)));
        // Independent heartbeat task — learning's own refresh interval
        // can be many minutes long, far longer than the watchdog
        // tolerance. Send a 30s heartbeat so the watchdog never trips
        // just because we're between policy refreshes.
        {
            let bus_hb = bus.clone();
            tokio::spawn(async move {
                let mut hb = tokio::time::interval(Duration::from_secs(30));
                loop {
                    hb.tick().await;
                    bus_hb.publish(AgentEvent::Heartbeat {
                        from: AgentId::Learning,
                        ts: Utc::now(),
                    });
                }
            });
        }
        // First tick fires immediately; if the journal is empty the
        // policy simply stays empty.
        loop {
            tick.tick().await;
            match journal.closed_trades(500) {
                Ok(trades) => {
                    let mem = PerformanceMemory::build(&trades);
                    let lessons = extractor.extract(&mem);
                    info!(
                        trades = trades.len(),
                        lessons = lessons.len(),
                        "learning agent: policy refreshed"
                    );
                    let count = lessons.len();
                    policy.update(mem, lessons);
                    bus.publish(AgentEvent::PolicyRefreshed {
                        lessons_count: count,
                        ts: Utc::now(),
                    });
                }
                Err(e) => warn!(error = %e, "learning agent: refresh failed"),
            }
        }
    })
}
