//! Layer 3 — LLM analysis & decision engine.

pub mod context_builder;
pub mod engine;
pub mod prompts;
pub mod response_parser;

pub use context_builder::{ContextBuilder, MarketContext};
pub use engine::{Decision, LlmEngine, TradeDecision};
