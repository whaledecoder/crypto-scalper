//! Claude-style LLM engine with timeout + fallback.

use crate::errors::{Result, ScalperError};
use crate::llm::context_builder::MarketContext;
use crate::llm::prompts::ARIA_SYSTEM_PROMPT;
use crate::llm::response_parser::parse_trade_decision;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    Go,
    NoGo,
    Wait,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeDecision {
    pub decision: Decision,
    pub direction: String,
    pub confidence: u8,
    pub entry_price: Option<f64>,
    pub sl_adjustment: Option<f64>,
    pub tp_adjustment: Option<f64>,
    pub reasoning: DecisionReasoning,
    pub market_context_score: ContextScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionReasoning {
    pub summary: String,
    pub ta_analysis: String,
    pub sentiment_analysis: String,
    pub fundamental_analysis: String,
    pub risk_factors: String,
    pub invalidation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ContextScore {
    pub ta_score: u8,
    pub sentiment_score: u8,
    pub fundamental_score: u8,
    pub risk_score: u8,
    pub composite_score: u8,
}

#[derive(Debug, Clone)]
pub struct LlmEngineConfig {
    pub api_key: String,
    pub api_base: String,
    pub model: String,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub fallback_ta_threshold: u8,
}

pub struct LlmEngine {
    client: Client,
    cfg: LlmEngineConfig,
}

pub struct LlmCallResult {
    pub decision: TradeDecision,
    pub latency_ms: u64,
    pub offline_fallback: bool,
}

impl LlmEngine {
    pub fn new(cfg: LlmEngineConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs + 2))
            .user_agent("ARIA-Scalper/0.1")
            .build()
            .unwrap_or_default();
        Self { client, cfg }
    }

    pub async fn analyze(&self, ctx: &MarketContext) -> Result<LlmCallResult> {
        let t0 = Instant::now();
        if self.cfg.api_key.is_empty() {
            warn!("LLM api key empty — running TA-only fallback");
            return Ok(LlmCallResult {
                decision: Self::fallback_decision(ctx, self.cfg.fallback_ta_threshold),
                latency_ms: 0,
                offline_fallback: true,
            });
        }

        let prompt = ctx.build_prompt();

        match timeout(
            Duration::from_secs(self.cfg.timeout_secs),
            self.call_api(&prompt),
        )
        .await
        {
            Ok(Ok(d)) => Ok(LlmCallResult {
                decision: d,
                latency_ms: t0.elapsed().as_millis() as u64,
                offline_fallback: false,
            }),
            Ok(Err(e)) => {
                warn!(error = %e, "LLM call failed — fallback");
                Ok(LlmCallResult {
                    decision: Self::fallback_decision(ctx, self.cfg.fallback_ta_threshold),
                    latency_ms: t0.elapsed().as_millis() as u64,
                    offline_fallback: true,
                })
            }
            Err(_) => {
                warn!("LLM timeout — fallback");
                Ok(LlmCallResult {
                    decision: Self::fallback_decision(ctx, self.cfg.fallback_ta_threshold),
                    latency_ms: t0.elapsed().as_millis() as u64,
                    offline_fallback: true,
                })
            }
        }
    }

    async fn call_api(&self, prompt: &str) -> Result<TradeDecision> {
        let body = serde_json::json!({
            "model": self.cfg.model,
            "max_tokens": self.cfg.max_tokens,
            "system": ARIA_SYSTEM_PROMPT,
            "messages": [{ "role": "user", "content": prompt }]
        });

        let resp: serde_json::Value = self
            .client
            .post(&self.cfg.api_base)
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        let text = resp
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| ScalperError::Llm("empty response".into()))?;

        info!(llm_raw = %text, "LLM response");
        parse_trade_decision(text)
    }

    fn fallback_decision(ctx: &MarketContext, threshold: u8) -> TradeDecision {
        let go = ctx.ta_confidence >= threshold;
        TradeDecision {
            decision: if go { Decision::Go } else { Decision::NoGo },
            direction: if go {
                ctx.pre_signal_direction.clone()
            } else {
                "NONE".into()
            },
            confidence: ctx.ta_confidence,
            entry_price: None,
            sl_adjustment: None,
            tp_adjustment: None,
            reasoning: DecisionReasoning {
                summary: "LLM unavailable — TA-only fallback mode".into(),
                ta_analysis: format!("TA confidence: {}/100", ctx.ta_confidence),
                sentiment_analysis: "N/A (LLM offline)".into(),
                fundamental_analysis: "N/A (LLM offline)".into(),
                risk_factors: format!("LLM offline — raised TA threshold to {threshold}+"),
                invalidation: "Any TA signal reversal".into(),
            },
            market_context_score: ContextScore {
                ta_score: ctx.ta_confidence,
                sentiment_score: 0,
                fundamental_score: 0,
                risk_score: 50,
                composite_score: ctx.ta_confidence,
            },
        }
    }
}
