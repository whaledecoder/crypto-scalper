//! Parse Anthropic-style responses into a `TradeDecision`, tolerating
//! common quirks (markdown fences, leading prose, BOM).

use crate::errors::{Result, ScalperError};
use crate::llm::engine::TradeDecision;

pub fn parse_trade_decision(raw: &str) -> Result<TradeDecision> {
    let cleaned = clean(raw);
    serde_json::from_str::<TradeDecision>(&cleaned)
        .map_err(|e| ScalperError::Parse(format!("llm json parse: {e}; payload: {cleaned}")))
}

fn clean(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('\u{feff}');
    let trimmed = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // If there's surrounding prose, try to extract the outermost braces.
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return trimmed[start..=end].to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_json() {
        let raw = r#"Here is my analysis:
```json
{
  "decision": "GO",
  "direction": "LONG",
  "confidence": 75,
  "entry_price": 67240.0,
  "sl_adjustment": null,
  "tp_adjustment": null,
  "reasoning": {
    "summary": "s",
    "ta_analysis": "t",
    "sentiment_analysis": "n",
    "fundamental_analysis": "f",
    "risk_factors": "r",
    "invalidation": "i"
  },
  "market_context_score": {
    "ta_score": 70,
    "sentiment_score": 70,
    "fundamental_score": 70,
    "risk_score": 70,
    "composite_score": 70
  }
}
```
"#;
        let d = parse_trade_decision(raw).unwrap();
        assert_eq!(d.confidence, 75);
    }
}
