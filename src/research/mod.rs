pub mod decay;
pub mod export;
pub mod ic;
pub mod report;
pub mod sensitivity;
pub mod significance;
pub mod walk_forward;

pub use decay::{compute_ic_decay, SignalObservation};
pub use export::{reports_to_json, reports_to_markdown, ResearchReport};
pub use ic::IcTracker;
pub use report::{StrategyHealth, StrategyResearchSummary};
pub use sensitivity::{summarize_parameter_sensitivity, ParameterPoint, SensitivitySummary};
pub use significance::{permutation_p_value, win_rate_significance};
pub use walk_forward::{
    evaluate_walk_forward, walk_forward_splits, WalkForwardResult, WalkForwardSplit,
    WalkForwardWindow,
};
