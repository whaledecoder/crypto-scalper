//! ARIA — LLM-Powered Autonomous Crypto Scalper.
//!
//! Library root. Re-exports the public modules so the binary and tests can use
//! them uniformly.

pub mod agents;
pub mod backtest;
pub mod config;
pub mod data;
pub mod errors;
pub mod execution;
pub mod feeds;
pub mod indicators;
pub mod learning;
pub mod llm;
pub mod monitoring;
pub mod strategy;
