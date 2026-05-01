//! On-chain metrics (exchange flow, whale txns, SOPR).
//! Optional — returns empty snapshot if no API key.

use chrono::{Duration as ChronoDuration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnchainSnapshot {
    pub symbol: String,
    pub exchange_inflow_24h: Option<f64>,
    pub exchange_outflow_24h: Option<f64>,
    pub whale_tx_1h: Option<u32>,
    pub sopr_1h: Option<f64>,
}

pub struct OnchainClient {
    client: Client,
    glassnode_key: Option<String>,
    whale_alert_key: Option<String>,
    glassnode_base_url: String,
    whale_alert_base_url: String,
}

impl OnchainClient {
    pub fn new(glassnode_key: Option<String>, whale_alert_key: Option<String>) -> Self {
        Self::with_base_urls(
            glassnode_key,
            whale_alert_key,
            "https://api.glassnode.com",
            "https://api.whale-alert.io",
        )
    }

    pub fn with_base_urls(
        glassnode_key: Option<String>,
        whale_alert_key: Option<String>,
        glassnode_base_url: impl Into<String>,
        whale_alert_base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            glassnode_key,
            whale_alert_key,
            glassnode_base_url: glassnode_base_url.into(),
            whale_alert_base_url: whale_alert_base_url.into(),
        }
    }

    pub async fn fetch(&self, symbol_upper: &str) -> anyhow::Result<OnchainSnapshot> {
        let asset = OnchainAsset::from_symbol(symbol_upper);
        let (exchange_flows, sopr, whale_tx) = if let Some(asset) = asset {
            let glassnode_flows = self.fetch_glassnode_exchange_flows(asset);
            let glassnode_sopr = self.fetch_glassnode_sopr(asset);
            let whale_alert = self.fetch_whale_alert_count(asset);
            tokio::join!(glassnode_flows, glassnode_sopr, whale_alert)
        } else {
            (None, None, None)
        };

        let (exchange_inflow_24h, exchange_outflow_24h) = exchange_flows.unwrap_or((None, None));

        Ok(OnchainSnapshot {
            symbol: symbol_upper.to_string(),
            exchange_inflow_24h,
            exchange_outflow_24h,
            whale_tx_1h: whale_tx,
            sopr_1h: sopr,
        })
    }

    async fn fetch_glassnode_exchange_flows(
        &self,
        asset: OnchainAsset,
    ) -> Option<(Option<f64>, Option<f64>)> {
        let inflow = self.fetch_glassnode_metric(
            asset,
            "/v1/metrics/transactions/transfers_volume_to_exchanges_sum",
            "24h",
        );
        let outflow = self.fetch_glassnode_metric(
            asset,
            "/v1/metrics/transactions/transfers_volume_from_exchanges_sum",
            "24h",
        );
        let (inflow, outflow) = tokio::join!(inflow, outflow);
        if inflow.is_some() || outflow.is_some() {
            Some((inflow, outflow))
        } else {
            None
        }
    }

    async fn fetch_glassnode_metric(
        &self,
        asset: OnchainAsset,
        path: &str,
        interval: &str,
    ) -> Option<f64> {
        let key = self.glassnode_key.as_ref().filter(|x| !x.is_empty())?;
        let url = format!("{}{}", self.glassnode_base_url.trim_end_matches('/'), path);
        let value = self
            .client
            .get(url)
            .query(&[
                ("api_key", key.as_str()),
                ("a", asset.glassnode_asset()),
                ("i", interval),
            ])
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<Value>()
            .await
            .ok()?;
        latest_metric_value(&value)
    }

    async fn fetch_glassnode_sopr(&self, asset: OnchainAsset) -> Option<f64> {
        self.fetch_glassnode_metric(asset, "/v1/metrics/indicators/sopr_adjusted", "1h")
            .await
    }

    async fn fetch_whale_alert_count(&self, asset: OnchainAsset) -> Option<u32> {
        let key = self.whale_alert_key.as_ref().filter(|x| !x.is_empty())?;
        let start = (Utc::now() - ChronoDuration::hours(1))
            .timestamp()
            .to_string();
        let url = format!(
            "{}/v1/transactions",
            self.whale_alert_base_url.trim_end_matches('/')
        );
        let value = self
            .client
            .get(url)
            .query(&[
                ("api_key", key.as_str()),
                ("start", start.as_str()),
                ("min_value", "1000000"),
                ("currency", asset.whale_alert_currency()),
            ])
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<Value>()
            .await
            .ok()?;
        parse_whale_tx_count(&value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnchainAsset {
    Btc,
    Eth,
}

impl OnchainAsset {
    fn from_symbol(symbol: &str) -> Option<Self> {
        let upper = symbol.to_ascii_uppercase();
        if upper.starts_with("BTC") {
            Some(Self::Btc)
        } else if upper.starts_with("ETH") {
            Some(Self::Eth)
        } else {
            None
        }
    }

    fn glassnode_asset(self) -> &'static str {
        match self {
            Self::Btc => "BTC",
            Self::Eth => "ETH",
        }
    }

    fn whale_alert_currency(self) -> &'static str {
        match self {
            Self::Btc => "btc",
            Self::Eth => "eth",
        }
    }
}

fn latest_metric_value(value: &Value) -> Option<f64> {
    value
        .as_array()?
        .iter()
        .rev()
        .find_map(|item| metric_value(item.get("v")?))
}

fn metric_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
        .filter(|x| x.is_finite())
}

fn parse_whale_tx_count(value: &Value) -> Option<u32> {
    value
        .get("count")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| {
            value
                .get("transactions")
                .and_then(|v| v.as_array())
                .and_then(|items| u32::try_from(items.len()).ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_supported_onchain_assets() {
        assert_eq!(
            OnchainAsset::from_symbol("BTCUSDT"),
            Some(OnchainAsset::Btc)
        );
        assert_eq!(
            OnchainAsset::from_symbol("ETHUSDT"),
            Some(OnchainAsset::Eth)
        );
        assert_eq!(OnchainAsset::from_symbol("SOLUSDT"), None);
    }

    #[test]
    fn parses_latest_glassnode_metric_value() {
        let payload = json!([
            {"t": 1, "v": "12.5"},
            {"t": 2, "v": 15.0}
        ]);
        assert_eq!(latest_metric_value(&payload), Some(15.0));
    }

    #[test]
    fn parses_whale_alert_count_or_transactions() {
        assert_eq!(parse_whale_tx_count(&json!({"count": 7})), Some(7));
        assert_eq!(
            parse_whale_tx_count(&json!({"transactions": [{}, {}, {}]})),
            Some(3)
        );
    }
}
