//! Learning agent — periodically rebuilds the `LearningPolicy` from the
//! trade journal and broadcasts the refresh event.
//!
//! Also feeds closed trade PnL into the QuantEngine for Kelly sizing.

use crate::agents::messages::{AgentEvent, AgentId};
use crate::agents::MessageBus;
use crate::learning::{
    lessons::{LessonConfig, LessonExtractor},
    LearningPolicy, PerformanceMemory,
};
use crate::monitoring::TradeJournal;
use crate::quant::QuantEngine;
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
    quant_engine: Option<Arc<QuantEngine>>,
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
        // Also listen for PositionClosed events to feed the quant engine
        // in real-time (don't wait for the 5-min refresh).
        if let Some(ref qe) = quant_engine {
            let qe_rt = Arc::clone(qe);
            let bus_rt = bus.clone();
            tokio::spawn(async move {
                let mut rx = bus_rt.subscribe();
                while let Ok(ev) = rx.recv().await {
                    if let AgentEvent::PositionClosed { pnl_usd, .. } = ev {
                        qe_rt.record_trade(pnl_usd);
                    }
                    if let AgentEvent::Shutdown = ev {
                        break;
                    }
                }
            });
        }

        // First tick fires immediately; if the journal is empty the
        // policy simply stays empty.
        loop {
            tick.tick().await;
            match journal.closed_trades(500) {
                Ok(trades) => {
                    // Feed all historical trade outcomes into the quant
                    // engine so Kelly has data from day 1.
                    if let Some(ref qe) = quant_engine {
                        for t in &trades {
                            qe.record_trade(t.pnl_usd);
                        }
                    }

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
                Err(e) => {
                    warn!(error = %e, "learning agent: failed to read journal");
                }
            }
        }
    })
}
