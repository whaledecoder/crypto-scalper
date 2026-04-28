//! Binance Futures funding rate & open interest.

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingSnapshot {
    pub symbol: String,
    pub rate: f64,
    pub predicted_rate: Option<f64>,
    pub open_interest: Option<f64>,
}

pub struct FundingClient {
    client: Client,
    base_url: String,
}

impl FundingClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
        }
    }

    pub async fn fetch(&self, symbol: &str) -> anyhow::Result<FundingSnapshot> {
        let premium_url = format!(
            "{}/fapi/v1/premiumIndex?symbol={}",
            self.base_url.trim_end_matches('/'),
            symbol
        );
        let premium: serde_json::Value = self.client.get(&premium_url).send().await?.json().await?;
        let rate: f64 = premium
            .get("lastFundingRate")
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);

        let oi_url = format!(
            "{}/fapi/v1/openInterest?symbol={}",
            self.base_url.trim_end_matches('/'),
            symbol
        );
        let oi: Option<f64> = match self.client.get(&oi_url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(r) => match r.json::<serde_json::Value>().await {
                    Ok(v) => v
                        .get("openInterest")
                        .and_then(|x| x.as_str())
                        .and_then(|s| s.parse::<f64>().ok()),
                    Err(_) => None,
                },
                Err(_) => None,
            },
            Err(_) => None,
        };

        Ok(FundingSnapshot {
            symbol: symbol.to_string(),
            rate,
            predicted_rate: None,
            open_interest: oi,
        })
    }
}
