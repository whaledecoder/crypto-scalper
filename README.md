# ARIA — Autonomous Realtime Intelligence Analyst

LLM-powered autonomous crypto scalping bot, written in Rust. ARIA combines
deterministic technical analysis with a multi-agent runtime where every
layer of the stack runs as an independent `tokio` task communicating
over a typed `MessageBus`. A `TraderManagerAgent` (second LLM) gives the
final approve/veto/adjust verdict on every trade, and a `SurvivalAgent`
polices the bot's own equity so it stays alive long enough to keep
trading.

> **Trade for life.** The default bias is *Veto*. The default behaviour
> on any LLM error is *Veto*. The default behaviour on any equity
> breach is *flat all positions and freeze*. Survival > opportunity.

```
┌──────────────────────────────────────────────────────────────────────┐
│ 12-agent runtime, all on a single tokio::sync::broadcast bus         │
│                                                                      │
│  Data → Signal → Risk → Brain → Manager → Execution                  │
│   │       │       │       │        │           │                     │
│   └─── Feeds ─────┘       │        │           │                     │
│                           │        │           │                     │
│  Learning ────────────────┘        │           │                     │
│                                    │           │                     │
│  Survival ──── broadcasts SurvivalUpdated to every agent ────────────┤
│  Control  ──── Telegram + /tmp/aria.control ingress  ────────────────┤
│  Watchdog ──── heartbeat dead-man-switch  ───────────────────────────┘
│  Monitor  ──── SQLite journal · Telegram · /metrics, /lessons,       │
│                /survival, /dashboard HTTP                            │
└──────────────────────────────────────────────────────────────────────┘
```

## Documentation Index

- **[INSTALL.md](INSTALL.md)** — step-by-step installation: prerequisites,
  build, configure, paper / live / backtest, troubleshooting.
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — full architecture
  (12 agents, MessageBus, AgentEvent enum, message flow per scenario).
- **[docs/SURVIVAL.md](docs/SURVIVAL.md)** — `survive_score` formula,
  `SurvivalMode` transitions, cooldown windows, death line, ratchet.
- **[docs/CONTROL.md](docs/CONTROL.md)** — Telegram commands, file
  ingress, watchdog, `/survival` & `/dashboard` HTTP endpoints.
- **[docs/CONFIG.md](docs/CONFIG.md)** — every config section with
  defaults and meaning.

## Highlights

- **6 logical layers, 12 agents** — Data, Feeds, Signal, Risk, Brain,
  Manager, Execution, Monitor, Learning, Survival, Control, Watchdog.
- **LLM via OpenRouter by default** — one key (`OPENROUTER_API_KEY`)
  unlocks Claude / GPT / Gemini / DeepSeek / Qwen. Anthropic native,
  OpenAI, Together, and Groq are all supported via the same engine.
- **Conservative risk gates** — entries must pass spread, reward:risk,
  transaction-cost-adjusted edge, daily-loss, drawdown, position-count,
  and notional-cap checks before any LLM/manager approval can reach execution.
- **Broker-side risk** — every entry pushes a paired `STOP_MARKET` +
  `TAKE_PROFIT_MARKET` to Binance with `closePosition=true`, so the
  position exits even if the bot crashes.
- **Idempotent client_order_id** — deterministic hash of
  `(symbol, strategy, side, entry, size, 1-minute bucket)`. Retries
  are de-duplicated by the exchange instead of producing duplicates.
- **Equity reconciliation** — `Exchange::fetch_equity_usd()` polled
  every 60 s. The in-memory `RiskManager` is always synced to broker truth.
- **Position reconciliation at startup** — restarted bot picks up
  positions that were already open at the broker via
  `fetch_open_positions()` + `PositionBook::reconcile`.
- **SurvivalAgent** — computes a 0–100 `survive_score` from drawdown,
  daily loss, loss streaks, news regime, and equity-floor proximity.
  Translates that into `SurvivalMode { Healthy, Cautious, Defensive,
  Frozen, Dead }` with size multipliers (1.0 / 0.6 / 0.3 / 0 / 0).
- **Manager LLM fails CLOSED** — on timeout/error → `Veto`. When
  survival mode is `Frozen` or `Dead`, the LLM is short-circuited
  with an instant Veto.
- **Operator control panel** — `/status`, `/positions`, `/freeze`,
  `/unfreeze`, `/flat`, `/health` over Telegram, plus a panic file at
  `/tmp/aria.control` (`echo flat >> /tmp/aria.control`).
- **Inter-agent watchdog** — every agent emits heartbeats; if any
  monitored agent goes silent past the threshold, `Watchdog` auto-issues
  a `Freeze` and unfreezes again when liveness returns.
- **Adaptive learning** — Layer 6 derives lessons (lose-streak,
  derate, boost, regime blacklist, LLM calibration, drawdown cooldown)
  from the SQLite journal every 5 minutes and feeds them back into
  every layer.

## Quick Start (Paper Mode, ~3 minutes)

> See **[INSTALL.md](INSTALL.md)** for a fuller walkthrough including
> live trading and Telegram setup.

```bash
# 1. Clone + build (Rust 1.85+)
git clone https://github.com/whaledecoder/crypto-scalper.git
cd crypto-scalper
cargo build --release

# 2. Get a free OpenRouter key (https://openrouter.ai/keys) and export it
cp .env.example .env
echo 'OPENROUTER_API_KEY=sk-or-v1-...' >> .env
set -a; source .env; set +a

# 3. Run in paper mode (default config — no real orders, no API keys needed
#    for the exchange).
./target/release/aria

# 4. Watch the dashboard
curl -s http://localhost:9184/dashboard | jq .
curl -s http://localhost:9184/survival  | jq .
```

The bot starts in **paper mode** by default — no real orders are sent.
Flip `[mode] run_mode = "live"` and `dry_run = false` only after you
have read [INSTALL.md](INSTALL.md) and verified everything in paper.

## The 12 Agents

| # | Agent | Owns | Reads | Emits |
|---|---|---|---|---|
| 1 | **DataAgent** | Binance WS, `OhlcvBuilder`, `OrderBook` | — | `Tick`, `BookTicker`, `CandleClosed` |
| 2 | **FeedsAgent** | F&G, funding, news, sentiment, on-chain | — | `FeedsSnapshot` |
| 3 | **SignalAgent** | regime detector + 5 strategies | candles, feeds, deadzone | `PreSignalEmitted` |
| 4 | **RiskAgent** | 8-gate `RiskManager`, learning policy, funding gate, survival hard-gate | pre-signal, feeds, survival | `RiskVerdict` |
| 5 | **BrainAgent** | `LlmEngine` (brain), `MarketContext` builder | risk verdict, feeds, history | `BrainOutcomeReady` |
| 6 | **TraderManagerAgent** | manager LLM, fail-closed handler | brain outcome, survival | `ManagerVerdictEmitted` |
| 7 | **ExecutionAgent** | `Exchange` impl, `PositionBook`, idempotent ID gen, broker-side SL/TP | manager verdict, control commands, survival | `OrderFilled`, `PositionClosed` |
| 8 | **MonitorAgent** | Prometheus snapshot, SQLite journal, Telegram alerts | every event | (write only) |
| 9 | **LearningAgent** | `LearningPolicy` refresh from journal | journal, position closes | `PolicyRefreshed` |
| 10 | **SurvivalAgent** | `survive_score`, equity reconciliation, cooldowns, death-line auto-flat | every relevant event | `SurvivalUpdated`, `EquityReconciled`, `ControlCommand::FlatAll` |
| 11 | **ControlAgent** | Telegram long-poll, `/tmp/aria.control` watcher, `RiskManager` sync | external commands | `ControlCommand::*` |
| 12 | **WatchdogAgent** | per-agent heartbeat tracker | `Heartbeat` events | `ControlCommand::Freeze` / `Unfreeze` |

## Configuration at a Glance

`config/default.toml` is the source of truth. Everything below has a
sensible default — you can run with no overlay at all in paper mode.

| Section | What it controls | See |
|---|---|---|
| `[mode]` | `run_mode` (paper / live / backtest), `dry_run` | [docs/CONFIG.md](docs/CONFIG.md#mode) |
| `[exchange]` | Binance URLs, `api_key`, `api_secret`, `recv_window_ms` | [docs/CONFIG.md](docs/CONFIG.md#exchange) |
| `[pairs]` | `symbols`, `timeframes` | [docs/CONFIG.md](docs/CONFIG.md#pairs) |
| `[strategy]` | active strategies, TA confidence floor | [docs/CONFIG.md](docs/CONFIG.md#strategy) |
| `[llm]` | brain LLM provider, model, key, fallback | [docs/CONFIG.md](docs/CONFIG.md#llm) |
| `[manager]` | manager LLM (final verdict layer) | [docs/CONFIG.md](docs/CONFIG.md#manager) |
| `[risk]` | per-trade %, max positions, daily-loss / drawdown / leverage / spread / edge / notional caps, equity | [docs/CONFIG.md](docs/CONFIG.md#risk) |
| `[schedule]` | WIB dead-zone hours | [docs/CONFIG.md](docs/CONFIG.md#schedule) |
| `[feeds]` | external feed API keys + RSS list | [docs/CONFIG.md](docs/CONFIG.md#feeds) |
| `[monitoring]` | Telegram, log level, SQLite path, metrics bind | [docs/CONFIG.md](docs/CONFIG.md#monitoring) |
| `[backtest]` | data dir, time window, TCM costs, Sharpe/Sortino annualization | [docs/CONFIG.md](docs/CONFIG.md#backtest) |
| `[survival]` | death line, cooldowns, ratchet, news blackout | [docs/SURVIVAL.md](docs/SURVIVAL.md) |
| `[control]` | Telegram command panel, allow-listed user IDs | [docs/CONTROL.md](docs/CONTROL.md) |

Quant roadmap progress is tracked in [docs/QUANT_ROADMAP.md](docs/QUANT_ROADMAP.md).

### LLM provider matrix

| Provider | `provider =` | `api_base` | Auth | Env var |
|---|---|---|---|---|
| **OpenRouter** *(default)* | `"openrouter"` | `https://openrouter.ai/api/v1/chat/completions` | `Authorization: Bearer …` | `OPENROUTER_API_KEY` |
| Anthropic native | `"anthropic"` | `https://api.anthropic.com/v1/messages` | `x-api-key: …` | `ANTHROPIC_API_KEY` |
| OpenAI | `"openai"` | `https://api.openai.com/v1/chat/completions` | `Authorization: Bearer …` | `OPENAI_API_KEY` |
| Together | `"together"` | `https://api.together.xyz/v1/chat/completions` | `Authorization: Bearer …` | `TOGETHER_API_KEY` |
| Groq | `"groq"` | `https://api.groq.com/openai/v1/chat/completions` | `Authorization: Bearer …` | `GROQ_API_KEY` |

OpenRouter sample models (price ≈ in/out per 1M tokens):

| Model | Cost | Notes |
|---|---|---|
| `anthropic/claude-3.5-haiku` | $0.80 / $4 | Default — smart & fast |
| `anthropic/claude-3.5-sonnet` | $3 / $15 | Best quality |
| `openai/gpt-4o-mini` | $0.15 / $0.60 | Solid generalist |
| `deepseek/deepseek-chat` | $0.14 / $0.28 | Cheap + sharp on TA reasoning |
| `meta-llama/llama-3.3-70b-instruct` | $0.13 / $0.39 | Fast |
| `google/gemini-2.0-flash-exp:free` | **FREE** | Rate-limited, fine for paper testing |
| `qwen/qwen-2.5-72b-instruct:free` | **FREE** | Rate-limited |

## HTTP Endpoints

The dashboard server (default `0.0.0.0:9184`) exposes:

| Path | Returns |
|---|---|
| `/healthz` | `"ok"` plain text — for liveness probes |
| `/metrics` | `MetricsSnapshot` JSON (mode, equity, daily PnL, positions, LLM stats, lesson count) |
| `/lessons` | array of currently active `Lesson` records |
| `/survival` | latest `SurvivalState` (or 404 if not yet computed) |
| `/dashboard` | combined `{metrics, lessons, survival}` JSON |

Example:

```bash
curl -s http://localhost:9184/dashboard | jq .
# {
#   "metrics": { "equity": 5037.4, "daily_pnl": 12.4, "active_lessons": 2, ... },
#   "lessons": [ {"kind":"StrategyBoost","strategy":"ema_ribbon", ...}, ... ],
#   "survival": {
#       "score": 88,
#       "mode": "Healthy",
#       "size_multiplier": 1.0,
#       "equity_usd": 5037.4,
#       "death_line_usd": 3500.0,
#       "drawdown_pct": 0.0,
#       "consecutive_losses": 0,
#       "reasons": []
#   }
# }
```

## Operator Control Panel

> Full reference: **[docs/CONTROL.md](docs/CONTROL.md)**.

### Telegram (off by default)

```toml
[control]
telegram_commands_enabled = true
allowed_user_ids = [123456789]    # your Telegram user id; empty = lock down
poll_secs = 3
```

Then export `TELEGRAM_BOT_TOKEN`. Commands:

| Command | Effect |
|---|---|
| `/status` | equity, daily PnL, peak, drawdown, positions open, frozen state |
| `/positions` | list open positions with entry / SL / TP |
| `/freeze` | manual freeze — RiskManager rejects any new entry |
| `/unfreeze` | resume trading |
| `/flat` | close all positions at market (panic button) |
| `/health` | `❤️ OK` |
| `/help` / `/start` | shows the command list |

### File ingress (always on)

For shell-driven panics that don't need Telegram:

```bash
echo flat     >> /tmp/aria.control   # /flat equivalent
echo freeze   >> /tmp/aria.control   # /freeze
echo unfreeze >> /tmp/aria.control   # /unfreeze
```

The file is auto-truncated after each read.

## Survival Mode

> Full reference: **[docs/SURVIVAL.md](docs/SURVIVAL.md)**.

```
score = 100
       - drawdown_penalty (max 60)
       - daily_loss_penalty (max 40)
       - loss_streak_penalty (max 30)
       - news_penalty (panic 25 / euphoria 10)
       - equity_floor_penalty (within 5% of death = -30)

mode = if equity <= death_line              -> Dead     (size ×0)
       else if cooldown / ratchet / score<25-> Frozen   (size ×0)
       else if score < 50                   -> Defensive(size ×0.3)
       else if score < 80                   -> Cautious (size ×0.6)
       else                                 -> Healthy  (size ×1.0)
```

Cooldown windows:

| Trigger | Pause |
|---|---|
| 3 consecutive losses on a `(strategy, symbol)` | 30 min |
| 5 losses within 60 min (any symbol) | 4 hours |
| 10 losses in a single day | 24 hours |
| Daily PnL crosses **+2 %** | Lock half the gain (frozen until rebuilt) |
| Equity ≤ 0.70 × initial (death line) | **Dead** — auto-flat + permanent freeze |
| Drawdown ≥ 8 % from peak | Auto-flat all positions + `Frozen` |
| News score < −0.6 (panic) | Freeze 2 h |
| News score > +0.8 (euphoria) | Halve size |

## Modes

| Mode | Effect |
|---|---|
| `paper` | Full pipeline, simulated fills, no real orders. Default. |
| `live` | Dispatches real orders to Binance (requires keys + `dry_run = false`). |
| `backtest` | Replays CSVs from `[backtest] data_dir/<SYMBOL>.csv`. |

## Backtesting

Place historical candles at `data/historical/BTCUSDT.csv` with header:

```
open_time_ms,open,high,low,close,volume
```

Run with `[mode] run_mode = "backtest"`; you'll get a per-symbol report
with WR, profit factor, annualized Sharpe/Sortino, and max drawdown. The
simulator subtracts entry/exit fees, adverse slippage, and market-impact cost.
Backtest also prints a research report table with Monte Carlo drawdown and
health classification; set `ARIA_RESEARCH_REPORT_FORMAT=json` for JSON output.

## Project Layout

```
src/
├── config.rs            # TOML + ENV loader (with [survival] / [control] defaults)
├── data/                # Layer 1 — WS, OHLCV builder, order book
├── indicators/          # Incremental TA primitives (EMA, RSI, BB, ATR, ADX, VWAP, …)
├── microstructure/      # OFI, VPIN, toxicity analytics
├── strategy/            # Layer 2 — symbol state, regime detector, 5 strategies
├── research/            # IC/IR, decay, walk-forward, permutation significance
├── portfolio/           # Kelly, vol target, correlation, VaR/CVaR helpers
├── feeds/               # External feeds (F&G, funding, news, sentiment, on-chain)
├── llm/                 # Layer 3 — context builder, prompts, multi-provider engine
├── execution/           # Layer 4 — risk gates, paper/Binance exchanges, position book
├── monitoring/          # Layer 5 — SQLite journal, Telegram, HTTP metrics/dashboard
├── learning/            # Layer 6 — performance memory, lessons, policy
├── agents/              # 12-agent runtime (data/feeds/signal/risk/brain/manager/
│                        # execution/monitor/learning/survival/control/watchdog)
├── backtest/            # Replay engine + performance metrics
├── research/            # IC/IR, walk-forward, Monte Carlo/significance helpers
├── portfolio/           # Kelly, vol target, correlation, exposure, VaR/CVaR helpers
├── lib.rs               # Module re-exports
└── main.rs              # Multi-agent orchestrator binary `aria`
```

## Quality Gates

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings   # 0 warnings
cargo test --lib
cargo build --release
```

## Security Notes

- Never commit `config/*.toml` or `.env` with real secrets — the
  `.gitignore` already blocks `.env`, but TOML overlays are your
  responsibility.
- `paper` mode never talks to the exchange and cannot place orders.
- Risk limits are enforced **before** every order dispatch.
- Manager LLM fails **closed** — when the LLM is unreachable, every
  signal is vetoed.
- Survival mode `Frozen` or `Dead` short-circuits the LLM entirely
  with an instant Veto and refuses to open new positions.

## License

MIT
