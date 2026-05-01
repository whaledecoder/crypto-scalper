#[derive(Debug, Clone, Copy, Default)]
pub struct AltDataInputs {
    pub news_sentiment: f64,
    pub social_sentiment: f64,
    pub onchain_flow: f64,
    pub fear_greed: f64,
}

pub fn alternative_data_score(inputs: AltDataInputs) -> f64 {
    let fg = ((inputs.fear_greed - 50.0) / 50.0).clamp(-1.0, 1.0);
    (inputs.news_sentiment.clamp(-1.0, 1.0) * 0.30
        + inputs.social_sentiment.clamp(-1.0, 1.0) * 0.25
        + inputs.onchain_flow.clamp(-1.0, 1.0) * 0.25
        + fg * 0.20)
        .clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_alt_data_context() {
        let score = alternative_data_score(AltDataInputs {
            news_sentiment: 0.5,
            social_sentiment: 0.5,
            onchain_flow: 0.2,
            fear_greed: 70.0,
        });
        assert!(score > 0.0);
    }
}
