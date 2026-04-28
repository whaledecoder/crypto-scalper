//! Alternative.me Fear & Greed Index client.

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FearGreedSnapshot {
    pub value: u8,
    pub label: FearGreedLabel,
    pub avg_7d: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FearGreedLabel {
    ExtremeFear,
    Fear,
    Neutral,
    Greed,
    ExtremeGreed,
}

impl FearGreedLabel {
    pub fn from_value(v: u8) -> Self {
        match v {
            0..=24 => Self::ExtremeFear,
            25..=44 => Self::Fear,
            45..=54 => Self::Neutral,
            55..=74 => Self::Greed,
            _ => Self::ExtremeGreed,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExtremeFear => "EXTREME_FEAR",
            Self::Fear => "FEAR",
            Self::Neutral => "NEUTRAL",
            Self::Greed => "GREED",
            Self::ExtremeGreed => "EXTREME_GREED",
        }
    }
}

pub struct FearGreedClient {
    client: Client,
    base_url: String,
}

impl FearGreedClient {
    pub fn new() -> Self {
        Self::with_base("https://api.alternative.me/fng/")
    }

    pub fn with_base(base: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            base_url: base.into(),
        }
    }

    pub async fn fetch(&self) -> anyhow::Result<FearGreedSnapshot> {
        let url = format!("{}?limit=7", self.base_url);
        let resp: serde_json::Value = self.client.get(&url).send().await?.json().await?;
        let data = resp
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("missing data"))?;
        let latest = data
            .first()
            .ok_or_else(|| anyhow::anyhow!("empty series"))?;
        let value: u8 = latest
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        let avg: Option<u8> = if data.len() >= 7 {
            let sum: u32 = data
                .iter()
                .take(7)
                .filter_map(|x| x.get("value").and_then(|v| v.as_str()))
                .filter_map(|s| s.parse::<u8>().ok())
                .map(|n| n as u32)
                .sum();
            Some((sum / 7) as u8)
        } else {
            None
        };
        Ok(FearGreedSnapshot {
            value,
            label: FearGreedLabel::from_value(value),
            avg_7d: avg,
        })
    }
}

impl Default for FearGreedClient {
    fn default() -> Self {
        Self::new()
    }
}
