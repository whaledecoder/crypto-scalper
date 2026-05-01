pub mod correlation;
pub mod kelly;
pub mod var;
pub mod vol_target;

pub use correlation::pearson_correlation;
pub use kelly::kelly_fraction;
pub use var::{historical_cvar, historical_var};
pub use vol_target::volatility_target_multiplier;
