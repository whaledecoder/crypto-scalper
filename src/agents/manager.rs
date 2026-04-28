//! TraderManagerAgent — head-of-desk LLM specialist that approves,
//! vetoes, or adjusts every signal coming out of the BrainAgent.
//!
//! Inputs:
//!   - The BrainAgent's `TradeDecision` (already a Go).
//!   - The RiskAgent's verdict (size, lessons matched).
//!   - Active learning lessons (from the policy).
//!   - Recent feed snapshot (for narrative context).
//!   - Open positions count (for global risk feel).
//!
//! Outputs: `ManagerVerdict` with `Approve` | `Veto` | `Adjust(...)`.
//!
//! Disable the manager by leaving the API key empty — it then defaults
//! to `Approve` for every Go decision (zero extra cost).

use crate::agents::messages::{
    AgentEvent, BrainOutcome, ManagerAction, ManagerProposal, ManagerVerdict,
};
use crate::agents::MessageBus;
use crate::feeds::ExternalSnapshot;
use crate::learning::LearningPolicy;
use crate::llm::engine::Decision;
use parking_lot::RwLock as PlRwLock;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Configuration for the manager LLM. Mirrors `LlmEngineConfig` but kept
/// separate so the manager can use a different (cheaper / smarter) model
/// than the brain.
pub struct ManagerAgentConfig {
    pub enabled: bool,
    pub provider: String, // "openrouter" | "anthropic"
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub http_referer: Option<String>,
    pub http_app_title: Option<String>,
    /// Approve everything below this confidence delta without an LLM call.
    /// (Optimisation: if Brain conf >= this and no lessons matched,
    /// skip the manager call to save tokens.)
    pub fast_approve_min_conf: u8,
}

impl Default for ManagerAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openrouter".into(),
            api_base: "https://openrouter.ai/api/v1/chat/completions".into(),
            api_key: String::new(),
            model: "anthropic/claude-3.5-haiku".into(),
            timeout_secs: 6,
            max_tokens: 600,
            http_referer: None,
            http_app_title: None,
            fast_approve_min_conf: 90,
        }
    }
}

pub fn spawn(
    bus: MessageBus,
    cfg: ManagerAgentConfig,
    policy: LearningPolicy,
    feeds_cache: Arc<PlRwLock<HashMap<String, ExternalSnapshot>>>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    let client = Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.max(2)))
        .build()
        .expect("manager http client");
    let cfg = Arc::new(cfg);

    tokio::spawn(async move {
        info!(
            enabled = cfg.enabled,
            model = %cfg.model,
            "manager agent starting"
        );
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Shutdown => break,
                AgentEvent::BrainOutcomeReady(brain) => {
                    if brain.decision.decision != Decision::Go {
                        // Brain already said no — nothing for manager to do.
                        continue;
                    }
                    let proposal = build_proposal(&brain);
                    let started = Instant::now();
                    let manager_off = !cfg.enabled || cfg.api_key.is_empty();
                    let fast_approve = brain.decision.confidence >= cfg.fast_approve_min_conf
                        && brain.risk.matched_lessons.is_empty();
                    let action = if manager_off || fast_approve {
                        ManagerAction::Approve
                    } else {
                        match call_manager_llm(
                            &client,
                            &cfg,
                            &proposal,
                            &brain,
                            &policy,
                            &feeds_cache,
                        )
                        .await
                        {
                            Ok(a) => a,
                            Err(e) => {
                                warn!(error = %e, "manager LLM failed — defaulting to Approve");
                                ManagerAction::Approve
                            }
                        }
                    };
                    let latency = started.elapsed().as_millis() as u64;
                    info!(
                        symbol = %proposal.symbol,
                        action = ?action,
                        latency_ms = latency,
                        "manager verdict"
                    );
                    bus.publish(AgentEvent::ManagerVerdictEmitted(ManagerVerdict {
                        proposal,
                        action,
                        latency_ms: latency,
                        offline_fallback: !cfg.enabled || cfg.api_key.is_empty(),
                        brain_outcome: brain,
                    }));
                }
                _ => {}
            }
        }
    })
}

fn build_proposal(brain: &BrainOutcome) -> ManagerProposal {
    let signal = &brain.signal;
    ManagerProposal {
        symbol: signal.symbol.clone(),
        side: signal.side,
        strategy: signal.strategy.as_str().to_string(),
        regime: brain.regime.as_str().to_string(),
        entry: brain.decision.entry_price.unwrap_or(signal.entry),
        stop_loss: signal.stop_loss,
        take_profit: signal.take_profit,
        size: brain.risk.size,
        ta_confidence: signal.ta_confidence,
        llm_confidence: brain.decision.confidence,
    }
}

const MANAGER_SYSTEM_PROMPT: &str = r#"You are the Trader-Manager for ARIA, a multi-agent crypto scalping bot.
Your specialists have already done their work and produced a trade
proposal. Your job: APPROVE, VETO, or ADJUST.

Be conservative and capital-protective. Veto when:
- Reasoning contradicts itself
- A learning lesson clearly indicates this trade is statistically unfavourable
- Risk/reward is poor
- Multiple lessons stack up against this strategy/symbol

Adjust (instead of veto) when the trade is reasonable but should be sized smaller
or have tighter SL / wider TP for current conditions.

You MUST respond with a strict JSON object, no commentary, of the form:

{
  "action": "approve",
  "reason": "concise <120 chars rationale"
}

OR

{
  "action": "veto",
  "reason": "concise <120 chars rationale"
}

OR

{
  "action": "adjust",
  "size_multiplier": 0.5,
  "sl_offset_bps": -10.0,
  "tp_offset_bps": 5.0,
  "reason": "concise <120 chars rationale"
}

`size_multiplier` ∈ [0.1, 1.5]. `sl_offset_bps` and `tp_offset_bps` are
adjustments in basis points (negative = move SL closer / TP closer).
Output ONLY the JSON object."#;

fn build_manager_user_prompt(
    proposal: &ManagerProposal,
    brain: &BrainOutcome,
    policy: &LearningPolicy,
    feeds_cache: &PlRwLock<HashMap<String, ExternalSnapshot>>,
) -> String {
    let lessons = policy.active_lessons();
    let mut s = String::new();
    s.push_str("[PROPOSAL]\n");
    s.push_str(&format!(
        "  symbol={} side={} strategy={} regime={}\n",
        proposal.symbol,
        proposal.side.as_str(),
        proposal.strategy,
        proposal.regime
    ));
    s.push_str(&format!(
        "  entry={:.4} sl={:.4} tp={:.4} size={:.6}\n",
        proposal.entry, proposal.stop_loss, proposal.take_profit, proposal.size
    ));
    s.push_str(&format!(
        "  ta_conf={} llm_conf={}\n",
        proposal.ta_confidence, proposal.llm_confidence
    ));
    s.push_str(&format!(
        "  rr_target = {:.2}\n",
        ((proposal.take_profit - proposal.entry).abs())
            / (proposal.entry - proposal.stop_loss).abs().max(1e-9)
    ));

    s.push_str("\n[BRAIN AGENT REASONING]\n");
    s.push_str(&format!(
        "  decision={:?} ta_score={:.1} sentiment_score={:.1} fundamental_score={:.1} composite={:.1}\n",
        brain.decision.decision,
        brain.decision.market_context_score.ta_score,
        brain.decision.market_context_score.sentiment_score,
        brain.decision.market_context_score.fundamental_score,
        brain.decision.market_context_score.composite_score
    ));
    s.push_str(&format!(
        "  summary: {}\n",
        brain.decision.reasoning.summary
    ));
    s.push_str(&format!(
        "  risks: {}\n",
        brain.decision.reasoning.risk_factors
    ));
    s.push_str(&format!(
        "  invalidation: {}\n",
        brain.decision.reasoning.invalidation
    ));

    s.push_str("\n[RISK AGENT]\n");
    s.push_str(&format!(
        "  base_size_multiplier={:.2} ta_threshold_eff={} llm_floor_eff={} matched_lessons={:?}\n",
        brain.risk.size_multiplier,
        brain.risk.effective_ta_threshold,
        brain.risk.effective_llm_floor,
        brain.risk.matched_lessons
    ));

    s.push_str("\n[LEARNING AGENT]\n");
    let summary = policy.historical_summary(&proposal.strategy, &proposal.regime, &proposal.symbol);
    if summary.is_empty() {
        s.push_str("  no history yet — cold start\n");
    } else {
        for line in summary.lines() {
            s.push_str(&format!("  {line}\n"));
        }
    }
    if !lessons.is_empty() {
        s.push_str("  active lessons:\n");
        for l in lessons.iter().take(8) {
            s.push_str(&format!("    - {:?}: {}\n", l.kind, l.reason));
        }
    }

    s.push_str("\n[FEEDS AGENT]\n");
    let cache = feeds_cache.read();
    if let Some(snap) = cache.get(&proposal.symbol) {
        if let Some(fg) = &snap.fear_greed {
            s.push_str(&format!(
                "  fear&greed: {} ({})\n",
                fg.value,
                fg.label.as_str()
            ));
        }
        if let Some(f) = &snap.funding {
            s.push_str(&format!("  funding rate: {}\n", f.rate));
        }
        if let Some(news) = &snap.news {
            s.push_str(&format!(
                "  news: net_score={:+.2} ({} items)\n",
                news.net_score,
                news.items.len()
            ));
        }
        if let Some(sent) = &snap.sentiment {
            s.push_str(&format!("  social sentiment: {:.2}\n", sent.sentiment));
        }
    } else {
        s.push_str("  feeds unavailable\n");
    }

    s.push_str("\nDecide.\n");
    s
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum ManagerActionDe {
    Approve {
        #[serde(default)]
        #[allow(dead_code)]
        reason: String,
    },
    Veto {
        #[serde(default)]
        reason: String,
    },
    Adjust {
        size_multiplier: Option<f64>,
        sl_offset_bps: Option<f64>,
        tp_offset_bps: Option<f64>,
        #[serde(default)]
        reason: String,
    },
}

impl From<ManagerActionDe> for ManagerAction {
    fn from(value: ManagerActionDe) -> Self {
        match value {
            ManagerActionDe::Approve { .. } => ManagerAction::Approve,
            ManagerActionDe::Veto { reason } => ManagerAction::Veto { reason },
            ManagerActionDe::Adjust {
                size_multiplier,
                sl_offset_bps,
                tp_offset_bps,
                reason,
            } => ManagerAction::Adjust {
                size_multiplier: size_multiplier.unwrap_or(1.0).clamp(0.1, 1.5),
                sl_offset_bps: sl_offset_bps.unwrap_or(0.0).clamp(-50.0, 50.0),
                tp_offset_bps: tp_offset_bps.unwrap_or(0.0).clamp(-50.0, 50.0),
                reason,
            },
        }
    }
}

pub fn parse_manager_response(raw: &str) -> Option<ManagerAction> {
    let cleaned = strip_code_fences(raw);
    if let Ok(de) = serde_json::from_str::<ManagerActionDe>(&cleaned) {
        return Some(de.into());
    }
    // Fallback: scan for the first JSON object.
    let bytes = cleaned.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let slice = &cleaned[start..end?];
    serde_json::from_str::<ManagerActionDe>(slice)
        .ok()
        .map(Into::into)
}

fn strip_code_fences(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("```json").unwrap_or(s);
    let s = s.strip_prefix("```").unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim().to_string()
}

async fn call_manager_llm(
    client: &Client,
    cfg: &ManagerAgentConfig,
    proposal: &ManagerProposal,
    brain: &BrainOutcome,
    policy: &LearningPolicy,
    feeds_cache: &PlRwLock<HashMap<String, ExternalSnapshot>>,
) -> anyhow::Result<ManagerAction> {
    let user = build_manager_user_prompt(proposal, brain, policy, feeds_cache);

    let body: Value = if cfg.provider.eq_ignore_ascii_case("anthropic") {
        json!({
            "model": cfg.model,
            "max_tokens": cfg.max_tokens,
            "system": MANAGER_SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": user}]
        })
    } else {
        json!({
            "model": cfg.model,
            "max_tokens": cfg.max_tokens,
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": MANAGER_SYSTEM_PROMPT},
                {"role": "user", "content": user}
            ]
        })
    };

    let mut req = client.post(&cfg.api_base).json(&body);
    if cfg.provider.eq_ignore_ascii_case("anthropic") {
        req = req
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01");
    } else {
        req = req.bearer_auth(&cfg.api_key);
        if let Some(r) = &cfg.http_referer {
            req = req.header("HTTP-Referer", r);
        }
        if let Some(t) = &cfg.http_app_title {
            req = req.header("X-Title", t);
        }
    }

    let resp = req.send().await?;
    let status = resp.status();
    let raw = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("manager LLM HTTP {status}: {raw}");
    }

    // Both Anthropic and OpenAI-compatible put the text inside a known path.
    let v: Value = serde_json::from_str(&raw)?;
    let text = if cfg.provider.eq_ignore_ascii_case("anthropic") {
        v["content"][0]["text"].as_str().unwrap_or("").to_string()
    } else {
        v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    parse_manager_response(&text)
        .ok_or_else(|| anyhow::anyhow!("manager LLM response not parseable: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_approve() {
        let a = parse_manager_response(r#"{"action":"approve","reason":"strong setup"}"#).unwrap();
        assert!(matches!(a, ManagerAction::Approve));
    }

    #[test]
    fn parses_veto_with_fence() {
        let raw = "```json\n{\"action\":\"veto\",\"reason\":\"news risk\"}\n```";
        let a = parse_manager_response(raw).unwrap();
        match a {
            ManagerAction::Veto { reason } => assert_eq!(reason, "news risk"),
            _ => panic!("expected veto"),
        }
    }

    #[test]
    fn parses_adjust_clamped() {
        let raw = r#"{
            "action":"adjust",
            "size_multiplier":2.5,
            "sl_offset_bps":-200.0,
            "tp_offset_bps":15.0,
            "reason":"trim size"
        }"#;
        let a = parse_manager_response(raw).unwrap();
        match a {
            ManagerAction::Adjust {
                size_multiplier,
                sl_offset_bps,
                tp_offset_bps,
                ..
            } => {
                assert_eq!(size_multiplier, 1.5);
                assert_eq!(sl_offset_bps, -50.0);
                assert_eq!(tp_offset_bps, 15.0);
            }
            _ => panic!("expected adjust"),
        }
    }
}
