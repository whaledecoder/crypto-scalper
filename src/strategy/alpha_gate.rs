use crate::feeds::funding_arb::{classify_funding, FundingArbSignal};
use crate::feeds::{alt_data::alternative_data_score, alt_data::AltDataInputs};
use crate::strategy::kalman::KalmanTrend;

#[derive(Debug, Clone, Copy)]
pub struct AdvancedAlphaInputs {
    pub alt_data: AltDataInputs,
    pub funding_rate: f64,
    pub trend_score: f64,
    pub min_abs_score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaGateDecision {
    Allow,
    Reduce,
    Block,
}

pub fn advanced_alpha_gate(inputs: AdvancedAlphaInputs, signal_is_long: bool) -> AlphaGateDecision {
    let directional = if signal_is_long { 1.0 } else { -1.0 };
    let alt_score = alternative_data_score(inputs.alt_data) * directional;
    let trend_score = (inputs.trend_score / 100.0).clamp(-1.0, 1.0) * directional;
    let funding_penalty = match classify_funding(inputs.funding_rate, 1.0) {
        FundingArbSignal::ReceiveFunding => 0.1,
        FundingArbSignal::PayFundingOnlyWithStrongTrend => -0.1,
        FundingArbSignal::Neutral => 0.0,
    } * directional;
    let score = alt_score * 0.45 + trend_score * 0.45 + funding_penalty;
    if score >= inputs.min_abs_score {
        AlphaGateDecision::Allow
    } else if score <= -inputs.min_abs_score {
        AlphaGateDecision::Block
    } else {
        AlphaGateDecision::Reduce
    }
}

pub fn kalman_trend_score(prices: &[f64], process_noise: f64, measurement_noise: f64) -> f64 {
    let Some(first) = prices.first() else {
        return 0.0;
    };
    let mut trend = KalmanTrend::new(*first, process_noise, measurement_noise);
    for price in prices.iter().skip(1) {
        trend.update(*price);
    }
    trend.trend_score(*prices.last().unwrap_or(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_against_bad_context() {
        let decision = advanced_alpha_gate(
            AdvancedAlphaInputs {
                alt_data: AltDataInputs {
                    news_sentiment: -0.8,
                    social_sentiment: -0.7,
                    onchain_flow: -0.6,
                    fear_greed: 20.0,
                },
                funding_rate: -0.0003,
                trend_score: -50.0,
                min_abs_score: 0.2,
            },
            true,
        );
        assert_eq!(decision, AlphaGateDecision::Block);
    }
}
