//! Social sentiment (LunarCrush or fallback stub).

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentimentSnapshot {
    pub symbol: String,
    pub social_volume: u64,
    pub social_volume_change_pct: f64,
    pub galaxy_score: Option<f64>,
    /// Range -1.0 .. 1.0
    pub sentiment: f64,
    pub top_keywords: Vec<String>,
}

pub struct SentimentClient {
    client: Client,
    key: Option<String>,
    base: String,
}

impl SentimentClient {
    pub fn new(key: Option<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            key,
            base: "https://lunarcrush.com/api4/public".to_string(),
        }
    }

    pub async fn fetch(&self, symbol_upper: &str) -> anyhow::Result<SentimentSnapshot> {
        if let Some(k) = &self.key {
            if !k.is_empty() {
                let url = format!(
                    "{}/coins/{}/v1",
                    self.base,
                    symbol_upper.trim_end_matches("USDT").to_lowercase()
                );
                let resp: serde_json::Value = self
                    .client
                    .get(&url)
                    .bearer_auth(k)
                    .send()
                    .await?
                    .json()
                    .await?;
                let d = resp.get("data").cloned().unwrap_or(serde_json::json!({}));
                let vol = d
                    .get("social_volume_24h")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let change = d
                    .get("social_volume_change_24h")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let galaxy = d.get("galaxy_score").and_then(|v| v.as_f64());
                let sentiment =
                    d.get("sentiment").and_then(|v| v.as_f64()).unwrap_or(0.5) * 2.0 - 1.0;
                return Ok(SentimentSnapshot {
                    symbol: symbol_upper.to_string(),
                    social_volume: vol,
                    social_volume_change_pct: change,
                    galaxy_score: galaxy,
                    sentiment,
                    top_keywords: vec![],
                });
            }
        }
        // Fallback — no key available.
        Ok(SentimentSnapshot {
            symbol: symbol_upper.to_string(),
            social_volume: 0,
            social_volume_change_pct: 0.0,
            galaxy_score: None,
            sentiment: 0.0,
            top_keywords: vec![],
        })
    }
}
