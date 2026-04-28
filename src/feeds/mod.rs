//! External data feeds for fundamental / sentiment / on-chain context.

pub mod fear_greed;
pub mod funding;
pub mod news;
pub mod onchain;
pub mod sentiment;

pub use fear_greed::{FearGreedClient, FearGreedSnapshot};
pub use funding::{FundingClient, FundingSnapshot};
pub use news::{NewsClient, NewsItem, NewsSnapshot};
pub use onchain::{OnchainClient, OnchainSnapshot};
pub use sentiment::{SentimentClient, SentimentSnapshot};

use serde::{Deserialize, Serialize};

/// Aggregated fundamentals passed to the LLM along with TA data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalSnapshot {
    pub news: Option<NewsSnapshot>,
    pub sentiment: Option<SentimentSnapshot>,
    pub onchain: Option<OnchainSnapshot>,
    pub funding: Option<FundingSnapshot>,
    pub fear_greed: Option<FearGreedSnapshot>,
}
