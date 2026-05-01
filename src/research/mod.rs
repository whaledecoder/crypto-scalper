pub mod decay;
pub mod ic;
pub mod significance;
pub mod walk_forward;

pub use decay::{compute_ic_decay, SignalObservation};
pub use ic::IcTracker;
pub use significance::permutation_p_value;
pub use walk_forward::{walk_forward_splits, WalkForwardSplit};
