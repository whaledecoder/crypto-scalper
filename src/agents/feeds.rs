//! External-feeds agent — periodically polls fear&greed / funding /
//! news / sentiment / on-chain feeds and republishes the snapshot.

use crate::agents::messages::{AgentEvent, AgentId, FeedsSnapshotMsg};
use crate::agents::MessageBus;
use crate::feeds::{
    DeribitOptionsClient, ExternalSnapshot, FearGreedClient, FundingClient, NewsClient,
    OnchainClient, SentimentClient,
};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::info;

pub struct FeedsAgentDeps {
    pub fear_greed: Arc<FearGreedClient>,
    pub funding: Arc<FundingClient>,
    pub news: Arc<NewsClient>,
    pub sentiment: Arc<SentimentClient>,
    pub onchain: Arc<OnchainClient>,
    pub options: Arc<DeribitOptionsClient>,
}

pub fn spawn(
    bus: MessageBus,
    deps: FeedsAgentDeps,
    symbols: Vec<String>,
    poll_secs: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("feeds agent starting");
        let mut tick = tokio::time::interval(Duration::from_secs(poll_secs.max(15)));
        loop {
            tick.tick().await;
            for symbol in &symbols {
                let base_owned: String = symbol.trim_end_matches("USDT").to_string();
                let base_slice: [&str; 1] = [base_owned.as_str()];
                let (fg, news, sent, onc, fund, options) = tokio::join!(
                    deps.fear_greed.fetch(),
                    deps.news.fetch(&base_slice),
                    deps.sentiment.fetch(symbol),
                    deps.onchain.fetch(symbol),
                    deps.funding.fetch(symbol),
                    deps.options.fetch(symbol),
                );
                let snapshot = ExternalSnapshot {
                    fear_greed: fg.ok(),
                    news: news.ok(),
                    sentiment: sent.ok(),
                    onchain: onc.ok(),
                    funding: fund.ok(),
                    options: options.ok().flatten(),
                };
                bus.publish(AgentEvent::FeedsSnapshot(FeedsSnapshotMsg {
                    symbol: symbol.clone(),
                    snapshot,
                    ts: Utc::now(),
                }));
            }
            // Liveness heartbeat after each poll cycle, regardless of
            // whether any feed actually returned data.
            bus.publish(AgentEvent::Heartbeat {
                from: AgentId::Feeds,
                ts: Utc::now(),
            });
        }
    })
}
