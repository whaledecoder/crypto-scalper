//! Social sentiment (LunarCrush or fallback stub).

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        Self::with_base_url(key, "https://lunarcrush.com/api4/public")
    }

    pub fn with_base_url(key: Option<String>, base: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            key,
            base: base.into(),
        }
    }

    pub async fn fetch(&self, symbol_upper: &str) -> anyhow::Result<SentimentSnapshot> {
        if let Some(k) = &self.key {
            if !k.is_empty() {
                let asset = lunarcrush_asset(symbol_upper);
                let url = format!("{}/coins/{}/v1", self.base, asset.to_ascii_lowercase());
                let resp: Value = self
                    .client
                    .get(&url)
                    .bearer_auth(k)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                return Ok(parse_lunarcrush_snapshot(symbol_upper, &resp));
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

fn lunarcrush_asset(symbol_upper: &str) -> &str {
    symbol_upper
        .strip_suffix("BUSD")
        .or_else(|| symbol_upper.strip_suffix("USDT"))
        .or_else(|| symbol_upper.strip_suffix("USD"))
        .unwrap_or(symbol_upper)
}

fn parse_lunarcrush_snapshot(symbol_upper: &str, resp: &Value) -> SentimentSnapshot {
    let d = resp.get("data").unwrap_or(resp);
    let social_volume = first_u64(
        d,
        &[
            "social_volume_24h",
            "social_volume",
            "interactions_24h",
            "posts_active",
        ],
    )
    .unwrap_or(0);
    let social_volume_change_pct = first_f64(
        d,
        &[
            "social_volume_change_24h",
            "social_dominance_calc_24h_previous",
            "interactions_24h_percent_change",
        ],
    )
    .unwrap_or(0.0);
    let galaxy_score = first_f64(d, &["galaxy_score", "galaxy_score_previous"]);
    let sentiment = normalized_sentiment(d);
    let top_keywords = d
        .get("topic_rank")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("topic")
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .take(5)
        .collect();

    SentimentSnapshot {
        symbol: symbol_upper.to_string(),
        social_volume,
        social_volume_change_pct,
        galaxy_score,
        sentiment,
        top_keywords,
    }
}

fn normalized_sentiment(d: &Value) -> f64 {
    if let Some(value) = first_f64(d, &["sentiment"]) {
        return if (0.0..=1.0).contains(&value) {
            value * 2.0 - 1.0
        } else if (0.0..=100.0).contains(&value) {
            value / 50.0 - 1.0
        } else {
            value.clamp(-1.0, 1.0)
        };
    }
    if let Some(value) = first_f64(d, &["sentiment_score"]) {
        return (value / 50.0 - 1.0).clamp(-1.0, 1.0);
    }
    if let Some(rank) = first_f64(d, &["alt_rank_30d"]) {
        return ((100.0 - rank.clamp(1.0, 200.0)) / 100.0).clamp(-1.0, 1.0);
    }
    0.0
}

fn first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(json_f64))
        .filter(|x| x.is_finite())
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(json_u64))
}

fn json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_lunarcrush_assets() {
        assert_eq!(lunarcrush_asset("BTCUSDT"), "BTC");
        assert_eq!(lunarcrush_asset("ETHUSD"), "ETH");
        assert_eq!(lunarcrush_asset("SOLBUSD"), "SOL");
        assert_eq!(lunarcrush_asset("TUSDUSDT"), "TUSD");
        assert_eq!(lunarcrush_asset("BUSDUSDT"), "BUSD");
    }

    #[test]
    fn parses_lunarcrush_payload_variants() {
        let payload = json!({
            "data": {
                "social_volume_24h": "1234",
                "social_volume_change_24h": "12.5",
                "galaxy_score": 72.0,
                "sentiment": 0.75,
                "topic_rank": [{"topic": "etf"}, {"name": "btc"}]
            }
        });
        let snap = parse_lunarcrush_snapshot("BTCUSDT", &payload);
        assert_eq!(snap.social_volume, 1234);
        assert_eq!(snap.social_volume_change_pct, 12.5);
        assert_eq!(snap.galaxy_score, Some(72.0));
        assert_eq!(snap.sentiment, 0.5);
        assert_eq!(snap.top_keywords, vec!["etf", "btc"]);
    }

    #[test]
    fn alt_rank_sentiment_is_not_inverted() {
        let strong = parse_lunarcrush_snapshot("BTCUSDT", &json!({"data": {"alt_rank_30d": 1}}));
        let weak = parse_lunarcrush_snapshot("BTCUSDT", &json!({"data": {"alt_rank_30d": 150}}));
        assert!(strong.sentiment > 0.0);
        assert!(weak.sentiment < 0.0);
    }
}
