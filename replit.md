# ARIA — LLM-Powered Autonomous Crypto Scalper Bot

## Project Overview

ARIA (Autonomous Realtime Intelligence Analyst) is a Rust-based multi-agent autonomous crypto trading bot. It connects to Binance Futures, analyzes market data using technical indicators and LLM reasoning, and executes trades in paper or live mode.

## Architecture

- **Language**: Rust (edition 2021, requires >= 1.85)
- **Build system**: Cargo
- **Binary**: `aria` (entry point: `src/main.rs`)
- **Runtime**: Tokio async multi-agent system communicating over a typed `MessageBus`
- **Database**: SQLite (`trades.db`) via `rusqlite` (bundled)
- **HTTP dashboard**: Axum server on port **8080** (Replit) / 9184 (default)

## Agent Architecture

The bot runs these concurrent agents:
- **DataAgent** — WebSocket feed from Binance Futures
- **FeedsAgent** — External news/sentiment/on-chain feeds
- **SignalAgent** — Technical analysis strategies
- **RiskAgent** — Position sizing and risk controls
- **BrainAgent** — LLM decision maker (OpenRouter/Anthropic)
- **TraderManagerAgent** — Optional LLM oversight layer
- **ExecutionAgent** — Order dispatch (paper or live)
- **SurvivalAgent** — Drawdown/streak protection
- **MonitorAgent** — Telegram alerts
- **LearningAgent** — Lesson extraction from past trades
- **ControlAgent** — Operator commands (stdin + Telegram)
- **WatchdogAgent** — Heartbeat monitoring

## Configuration

Config is layered (merged in order):
1. `config/default.toml` — base defaults (paper mode)
2. `ARIA_CONFIG_OVERLAY` env var pointing to an overlay file
3. Environment variables for secrets

### Overlays provided
- `config/paper.toml` — paper trading safe mode
- `config/replit.toml` — **Replit-specific**: paper mode + port 8080
- `config/production.toml` — live mode, tighter risk caps
- `config/aggressive.toml` — higher risk settings
- `config/llm-anthropic.toml` — use Anthropic native API
- `config/llm-openrouter-cheap.toml` — cheap/free OpenRouter models

## Run Modes

- `paper` — simulated fills, no real orders (default / safe)
- `live` — real Binance Futures orders (requires API keys)
- `backtest` — replay historical CSVs from `data/historical/`

## Dashboard Endpoints (port 8080)

- `GET /` — welcome message
- `GET /healthz` — health check
- `GET /metrics` — trading metrics JSON
- `GET /dashboard` — full dashboard JSON
- `GET /survival` — survival/drawdown state JSON
- `GET /lessons` — active lessons JSON

## Environment Variables

See `.env.example` for the full list. Key vars:
- `BINANCE_API_KEY` / `BINANCE_API_SECRET` — required for live mode
- `OPENROUTER_API_KEY` — brain LLM (optional; falls back to TA-only)
- `ANTHROPIC_API_KEY` — if using Anthropic directly
- `TELEGRAM_BOT_TOKEN` / `TELEGRAM_CHAT_ID` — alerts
- `ARIA_CONFIG_OVERLAY` — path to config overlay
- `RUST_LOG` — log level (e.g. `info`, `debug`)

## Replit Workflow

**Workflow**: `Start application`
**Command**: `ARIA_CONFIG_OVERLAY=config/replit.toml ./target/debug/aria`
**Port**: 8080 (console output type)

The workflow runs the debug build for faster iteration. The deployment uses `cargo build --release` to produce an optimized binary.

## Building

```bash
# Debug build (fast)
cargo build

# Release build (optimized, ~2-3 min cold)
cargo build --release

# Run tests
cargo test --lib
```

## System Dependencies

Requires OpenSSL (installed via Nix `openssl` + `pkg-config`).

## Deployment

- **Target**: VM (always-running bot)
- **Build**: `cargo build --release`
- **Run**: `ARIA_CONFIG_OVERLAY=config/replit.toml ./target/release/aria`
