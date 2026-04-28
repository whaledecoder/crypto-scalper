//! Learning agent — periodically rebuilds the `LearningPolicy` from the
//! trade journal and broadcasts the refresh event.

use crate::agents::messages::AgentEvent;
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
