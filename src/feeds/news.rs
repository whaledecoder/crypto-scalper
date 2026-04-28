//! News aggregator. Combines CryptoPanic (if key set) with free RSS feeds.
//!
//! Each item is scored into `[-1.0, 1.0]` as a cheap keyword signal so the LLM
//! still has structure to work with even when the text is large.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_at: Option<String>,
    /// -1.0 negative, 0.0 neutral, +1.0 positive (keyword heuristic).
    pub score: f64,
    /// LOW | MED | HIGH
    pub impact: Impact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Impact {
    Low,
    Medium,
    High,
}

impl Impact {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::Medium => "MED",
            Self::High => "HIGH",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsSnapshot {
    pub items: Vec<NewsItem>,
    pub net_score: f64,
}

pub struct NewsClient {
    client: Client,
    cryptopanic_key: Option<String>,
    rss_urls: Vec<String>,
}

impl NewsClient {
    pub fn new(cryptopanic_key: Option<String>, rss_urls: Vec<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(6))
                .user_agent("ARIA-Scalper/0.1")
                .build()
                .unwrap_or_default(),
            cryptopanic_key,
            rss_urls,
        }
    }

    pub async fn fetch(&self, currencies: &[&str]) -> anyhow::Result<NewsSnapshot> {
        let mut items = Vec::new();

        if let Some(key) = &self.cryptopanic_key {
            if !key.is_empty() {
                if let Ok(mut cp) = self.fetch_cryptopanic(key, currencies).await {
                    items.append(&mut cp);
                }
            }
        }

        for url in &self.rss_urls {
            if let Ok(mut feed) = self.fetch_rss(url).await {
                items.append(&mut feed);
            }
        }

        // Keep only recent + dedupe by title.
        items.sort_by(|a, b| b.impact.as_str().cmp(a.impact.as_str()));
        items.dedup_by(|a, b| a.title == b.title);
        if items.len() > 20 {
            items.truncate(20);
        }

        let net = if items.is_empty() {
            0.0
        } else {
            items.iter().map(|i| i.score).sum::<f64>() / items.len() as f64
        };
        Ok(NewsSnapshot {
            items,
            net_score: net,
        })
    }

    async fn fetch_cryptopanic(
        &self,
        key: &str,
        currencies: &[&str],
    ) -> anyhow::Result<Vec<NewsItem>> {
        let curr = currencies.join(",");
        let url = format!(
            "https://cryptopanic.com/api/v1/posts/?auth_token={key}&currencies={curr}&public=true"
        );
        let body: serde_json::Value = self.client.get(&url).send().await?.json().await?;
        let arr = body
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .take(10)
            .map(|post| {
                let title = post
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = post
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let pub_at = post
                    .get("published_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let score = keyword_sentiment(&title);
                NewsItem {
                    source: "cryptopanic".into(),
                    title,
                    url,
                    published_at: pub_at,
                    score,
                    impact: classify_impact(score),
                }
            })
            .collect())
    }

    async fn fetch_rss(&self, url: &str) -> anyhow::Result<Vec<NewsItem>> {
        let bytes = self.client.get(url).send().await?.bytes().await?;
        let feed = feed_rs::parser::parse(bytes.as_ref())?;
        let source = feed
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_else(|| url.to_string());
        Ok(feed
            .entries
            .into_iter()
            .take(10)
            .map(|e| {
                let title = e.title.map(|t| t.content).unwrap_or_default();
                let link = e.links.first().map(|l| l.href.clone()).unwrap_or_default();
                let score = keyword_sentiment(&title);
                NewsItem {
                    source: source.clone(),
                    title,
                    url: link,
                    published_at: e.published.map(|p| p.to_rfc3339()),
                    score,
                    impact: classify_impact(score),
                }
            })
            .collect())
    }
}

fn keyword_sentiment(title: &str) -> f64 {
    let t = title.to_lowercase();
    let pos = [
        "etf",
        "approval",
        "inflow",
        "accumulation",
        "bullish",
        "surge",
        "rally",
        "breakout",
        "all-time high",
        "ath",
        "institution",
        "upgrade",
        "adoption",
    ];
    let neg = [
        "hack",
        "exploit",
        "lawsuit",
        "ban",
        "outflow",
        "selloff",
        "crash",
        "dump",
        "bearish",
        "sec",
        "regulator",
        "investigation",
        "fud",
        "liquidation",
        "exploit",
        "rug",
    ];
    let mut s: f64 = 0.0;
    for k in pos {
        if t.contains(k) {
            s += 1.0;
        }
    }
    for k in neg {
        if t.contains(k) {
            s -= 1.0;
        }
    }
    (s / 3.0).clamp(-1.0, 1.0)
}

fn classify_impact(score: f64) -> Impact {
    let a = score.abs();
    if a >= 0.66 {
        Impact::High
    } else if a >= 0.33 {
        Impact::Medium
    } else {
        Impact::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_sentiment_ranges() {
        assert!(keyword_sentiment("BlackRock BTC ETF inflow surges") > 0.0);
        assert!(keyword_sentiment("SEC lawsuit hits exchange") < 0.0);
        assert!(keyword_sentiment("Bitcoin holds steady").abs() < 1e-9);
    }
}
