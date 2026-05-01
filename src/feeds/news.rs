//! News aggregator. Combines CryptoPanic (if key set) with free RSS feeds.
//!
//! Each item is scored into `[-1.0, 1.0]` as a cheap keyword signal so the LLM
//! still has structure to work with even when the text is large.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    cryptopanic_base_url: String,
}

impl NewsClient {
    pub fn new(cryptopanic_key: Option<String>, rss_urls: Vec<String>) -> Self {
        Self::with_cryptopanic_base_url(
            cryptopanic_key,
            rss_urls,
            "https://cryptopanic.com/api/v1/posts/",
        )
    }

    pub fn with_cryptopanic_base_url(
        cryptopanic_key: Option<String>,
        rss_urls: Vec<String>,
        cryptopanic_base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(6))
                .user_agent("ARIA-Scalper/0.1")
                .build()
                .unwrap_or_default(),
            cryptopanic_key,
            rss_urls,
            cryptopanic_base_url: cryptopanic_base_url.into(),
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
        let body: Value = self
            .client
            .get(&self.cryptopanic_base_url)
            .query(&[
                ("auth_token", key),
                ("currencies", curr.as_str()),
                ("public", "true"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_cryptopanic_items(&body, 10))
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

fn parse_cryptopanic_items(body: &Value, limit: usize) -> Vec<NewsItem> {
    body.get("results")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .take(limit)
        .filter_map(parse_cryptopanic_item)
        .collect()
}

fn parse_cryptopanic_item(post: &Value) -> Option<NewsItem> {
    let title = post.get("title").and_then(|v| v.as_str())?;
    if title.is_empty() {
        return None;
    }
    let url = post
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pub_at = post
        .get("published_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut score = keyword_sentiment(title);
    if let Some(votes) = post.get("votes") {
        score = (score + cryptopanic_vote_score(votes)).clamp(-1.0, 1.0);
    }
    Some(NewsItem {
        source: "cryptopanic".into(),
        title: title.to_string(),
        url,
        published_at: pub_at,
        score,
        impact: classify_impact(score),
    })
}

fn cryptopanic_vote_score(votes: &Value) -> f64 {
    let positive = ["positive", "bullish"]
        .iter()
        .filter_map(|k| votes.get(k).and_then(|v| v.as_f64()))
        .sum::<f64>();
    let negative = ["negative", "bearish"]
        .iter()
        .filter_map(|k| votes.get(k).and_then(|v| v.as_f64()))
        .sum::<f64>();
    let total = positive + negative;
    if total > 0.0 {
        ((positive - negative) / total).clamp(-1.0, 1.0) * 0.5
    } else {
        0.0
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

    #[test]
    fn parses_cryptopanic_votes_into_score() {
        let payload = serde_json::json!({
            "results": [{
                "title": "BTC ETF rally",
                "url": "https://example.com/post",
                "published_at": "2026-01-01T00:00:00Z",
                "votes": {"positive": 9, "negative": 1, "important": 20}
            }]
        });
        let items = parse_cryptopanic_items(&payload, 10);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source, "cryptopanic");
        assert!(items[0].score > 0.5);
    }

    #[test]
    fn cryptopanic_vote_score_ignores_non_directional_votes() {
        let votes = serde_json::json!({
            "negative": 10,
            "important": 50,
            "liked": 50,
            "toxic": 50
        });
        assert_eq!(cryptopanic_vote_score(&votes), -0.5);
    }
}
