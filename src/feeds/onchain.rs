//! On-chain metrics (exchange flow, whale txns, SOPR).
//! Optional — returns empty snapshot if no API key.

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnchainSnapshot {
    pub symbol: String,
    pub exchange_inflow_24h: Option<f64>,
    pub exchange_outflow_24h: Option<f64>,
    pub whale_tx_1h: Option<u32>,
    pub sopr_1h: Option<f64>,
}

pub struct OnchainClient {
    _client: Client,
    _glassnode_key: Option<String>,
    _whale_alert_key: Option<String>,
}

impl OnchainClient {
    pub fn new(glassnode_key: Option<String>, whale_alert_key: Option<String>) -> Self {
        Self {
            _client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            _glassnode_key: glassnode_key,
            _whale_alert_key: whale_alert_key,
        }
    }

    /// Minimal stub — in production wire up Glassnode + Whale Alert endpoints.
    pub async fn fetch(&self, symbol_upper: &str) -> anyhow::Result<OnchainSnapshot> {
        Ok(OnchainSnapshot {
            symbol: symbol_upper.to_string(),
            exchange_inflow_24h: None,
            exchange_outflow_24h: None,
            whale_tx_1h: None,
            sopr_1h: None,
        })
    }
}
