use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OptionSkewSnapshot {
    pub underlying: OptionUnderlying,
    pub call_25d_iv: f64,
    pub put_25d_iv: f64,
    pub atm_iv: f64,
    pub sample_size: usize,
}

impl OptionSkewSnapshot {
    pub fn skew_bps(&self) -> f64 {
        if self.atm_iv <= 0.0 {
            return 0.0;
        }
        (self.call_25d_iv - self.put_25d_iv) / self.atm_iv * 10_000.0
    }

    pub fn sentiment_score(&self) -> f64 {
        (self.skew_bps() / 500.0).clamp(-1.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionUnderlying {
    Btc,
    Eth,
}

impl OptionUnderlying {
    pub fn from_symbol(symbol: &str) -> Option<Self> {
        let upper = symbol.to_ascii_uppercase();
        if upper.starts_with("BTC") {
            Some(Self::Btc)
        } else if upper.starts_with("ETH") {
            Some(Self::Eth)
        } else {
            None
        }
    }

    pub fn deribit_currency(self) -> &'static str {
        match self {
            Self::Btc => "BTC",
            Self::Eth => "ETH",
        }
    }
}

pub struct DeribitOptionsClient {
    client: Client,
    base_url: String,
}

impl DeribitOptionsClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
        }
    }

    pub async fn fetch(&self, symbol: &str) -> anyhow::Result<Option<OptionSkewSnapshot>> {
        let Some(underlying) = OptionUnderlying::from_symbol(symbol) else {
            return Ok(None);
        };
        let url = format!(
            "{}/api/v2/public/get_book_summary_by_currency?currency={}&kind=option",
            self.base_url.trim_end_matches('/'),
            underlying.deribit_currency()
        );
        let response: DeribitBookSummaryResponse =
            self.client.get(&url).send().await?.json().await?;
        Ok(derive_skew_snapshot(underlying, &response.result))
    }
}

#[derive(Debug, Deserialize)]
struct DeribitBookSummaryResponse {
    result: Vec<DeribitBookSummary>,
}

#[derive(Debug, Deserialize)]
struct DeribitBookSummary {
    instrument_name: String,
    mark_iv: Option<f64>,
}

fn derive_skew_snapshot(
    underlying: OptionUnderlying,
    rows: &[DeribitBookSummary],
) -> Option<OptionSkewSnapshot> {
    let mut calls = Vec::new();
    let mut puts = Vec::new();
    for row in rows {
        let Some(iv) = row.mark_iv else {
            continue;
        };
        if !iv.is_finite() || iv <= 0.0 {
            continue;
        }
        if row.instrument_name.ends_with("-C") {
            calls.push(iv / 100.0);
        } else if row.instrument_name.ends_with("-P") {
            puts.push(iv / 100.0);
        }
    }
    if calls.len() < 3 || puts.len() < 3 {
        return None;
    }
    calls.sort_by(|a, b| a.total_cmp(b));
    puts.sort_by(|a, b| a.total_cmp(b));
    let call_25d_iv = calls[calls.len() * 3 / 4];
    let put_25d_iv = puts[puts.len() * 3 / 4];
    let atm_iv = median(&calls, &puts);
    Some(OptionSkewSnapshot {
        underlying,
        call_25d_iv,
        put_25d_iv,
        atm_iv,
        sample_size: calls.len() + puts.len(),
    })
}

fn median(calls: &[f64], puts: &[f64]) -> f64 {
    let mut values = Vec::with_capacity(calls.len() + puts.len());
    values.extend_from_slice(calls);
    values.extend_from_slice(puts);
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_iv_skew() {
        let skew = OptionSkewSnapshot {
            underlying: OptionUnderlying::Btc,
            call_25d_iv: 0.65,
            put_25d_iv: 0.55,
            atm_iv: 0.60,
            sample_size: 10,
        };
        assert!(skew.skew_bps() > 0.0);
        assert!(skew.sentiment_score() > 0.0);
    }

    #[test]
    fn maps_supported_underlyings() {
        assert_eq!(
            OptionUnderlying::from_symbol("BTCUSDT"),
            Some(OptionUnderlying::Btc)
        );
        assert_eq!(
            OptionUnderlying::from_symbol("ETHUSDT"),
            Some(OptionUnderlying::Eth)
        );
        assert_eq!(OptionUnderlying::from_symbol("SOLUSDT"), None);
    }

    #[test]
    fn derives_skew_from_deribit_iv_rows() {
        let snapshot = derive_skew_snapshot(
            OptionUnderlying::Btc,
            &[
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-80000-C".into(),
                    mark_iv: Some(55.0),
                },
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-90000-C".into(),
                    mark_iv: Some(60.0),
                },
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-100000-C".into(),
                    mark_iv: Some(65.0),
                },
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-80000-P".into(),
                    mark_iv: Some(45.0),
                },
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-70000-P".into(),
                    mark_iv: Some(50.0),
                },
                DeribitBookSummary {
                    instrument_name: "BTC-1JAN27-60000-P".into(),
                    mark_iv: Some(52.0),
                },
            ],
        )
        .expect("enough rows");
        assert_eq!(snapshot.underlying, OptionUnderlying::Btc);
        assert_eq!(snapshot.sample_size, 6);
        assert!(snapshot.call_25d_iv > snapshot.put_25d_iv);
    }
}
