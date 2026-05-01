# Architecture

ARIA is a 12-agent runtime built on `tokio` tasks and a single typed
`tokio::sync::broadcast` `MessageBus`. There is no shared mutable state
between agents — every agent owns its own task-local state and communicates
exclusively by publishing or subscribing to `AgentEvent`s.

> The bus is intentionally **lossy** (capacity 4096). Agents that fall
> behind drop messages rather than blocking the producer. This is the
> right trade-off for a real-time scalper — staleness > stalls.

---

## 1. Bird's-eye view

```
                         ┌────────────────────────────┐
                         │    MessageBus (broadcast)  │
                         │       capacity = 4096      │
                         └─────────────┬──────────────┘
                                       │ AgentEvent
   ┌───────────────────────────────────┼───────────────────────────────────┐
   │                                   │                                   │
┌──▼──┐  ┌───────┐  ┌──────┐  ┌─────┐  │   ┌───────┐  ┌──────────┐  ┌───────┐
│Data │  │Feeds  │  │Signal│  │Risk │  │   │Brain  │  │ Manager  │  │ Exec  │
└──┬──┘  └───┬───┘  └──┬───┘  └──┬──┘  │   └───┬───┘  └────┬─────┘  └───┬───┘
   │ Tick   │ Snap     │ Pre-     │     │       │ Brain     │ Verdict    │ Filled/
   │ Candle │ shot     │ Signal   │ Risk│       │ Outcome   │            │ Closed
   ▼        ▼          ▼          ▼     ▼       ▼           ▼            ▼
   ────────────────────────────  bus  ──────────────────────────────────────
                                ▲   ▲   ▲  ▲   ▲  ▲
                                │   │   │  │   │  │ Heartbeat / Survival /
   ┌───────┐  ┌─────────┐  ┌────┴───┴───┴──┴───┴──┴──┐
   │Monitor│  │Learning │  │ Survival · Control · Watchdog │
   └───────┘  └─────────┘  └─────────────────────────────────┘
```

---

## 2. Layers vs Agents

ARIA's six logical layers map to the agents like this:

| Layer | Responsibility | Agent(s) |
|---|---|---|
| 1. Data | Real-time market data | `DataAgent` |
| — | External context feeds | `FeedsAgent` |
| 2. Signal | Indicators + regime + strategies | `SignalAgent` |
| 3. Brain | LLM decision engine + risk gates | `RiskAgent`, `BrainAgent`, `TraderManagerAgent` |
| 4. Execution | Order dispatch + position book | `ExecutionAgent` |
| 5. Monitoring | Journal + alerts + metrics | `MonitorAgent` |
| 6. Learning | Adaptive policy from history | `LearningAgent` |
| 7. Survival | Capital preservation | `SurvivalAgent` |
| 8. Control | Operator interface + watchdog | `ControlAgent`, `WatchdogAgent` |

---

## 3. The 12 agents in detail

### 3.1 `DataAgent` — `src/agents/data.rs`

Owns the Binance WebSocket connection (auto-reconnect with exponential
backoff). Drives the `OhlcvBuilder` and maintains a fresh `OrderBook`
snapshot. Emits:

- `Tick { symbol, trade }` — every aggTrade
- `BookTicker { symbol, best_bid, bid_qty, best_ask, ask_qty }`
- `CandleClosed { symbol, candle }` — once per closed bar

Heartbeat: every loop iteration when at least one event was emitted.

### 3.2 `FeedsAgent` — `src/agents/feeds.rs`

Periodic poller for external context:

- Fear & Greed Index (alternative.me, free)
- Funding rate + open interest (Binance, free)
- News (CryptoPanic optional + RSS list)
- Social sentiment (LunarCrush optional)
- On-chain flow (Glassnode + Whale Alert optional)

Emits `FeedsSnapshot { symbol, snapshot }` per polling cycle (default
60 s). Missing API keys → those sub-feeds are silently skipped.

### 3.3 `SignalAgent` — `src/agents/signal.rs`

Per-symbol `SymbolState` updated on every `CandleClosed`. Runs the
regime detector + the active strategies; if any produces a trade
candidate, emits `PreSignalEmitted { pre_signal, regime, ... }`.
Streaming `bookTicker` quantity deltas also update rolling order-flow
imbalance (OFI), which strategies can use as a microstructure
confirmation signal.

Skips processing when the WIB schedule is in the dead zone:

```rust
fn in_dead_zone(s: &Schedule) -> bool {
    let now_wib_hour = (Utc::now().hour() as i32 + 7).rem_euclid(24) as u8;
    if s.start < s.end {
        now_wib >= s.start && now_wib < s.end
    } else {
        now_wib >= s.start || now_wib < s.end    // wrap-aware
    }
}
```

### 3.4 `RiskAgent` — `src/agents/risk.rs`

Applies, in order:

1. Survival hard-gate — `Frozen` or `Dead` → `RiskOutcome::Blocked`.
2. 8-gate `RiskManager` — daily loss, drawdown, max positions, spread,
   leverage, equity floor, freeze/trip, sizing.
3. Learning policy — drops blacklisted `(strategy, symbol)`, applies
   size multipliers from active lessons.
4. Funding rate gate — blocks longs at funding ≥ +0.1 %, shorts at ≤ −0.1 %.
5. Final TA threshold check.

Emits `RiskVerdict { allowed | blocked, size, lessons[] }`.

### 3.5 `BrainAgent` — `src/agents/brain.rs`

Builds a `MarketContext` packet (technical state, regime, feeds
snapshot, lessons summary) and calls the brain LLM. On error/timeout,
falls back to TA-only confidence. Emits `BrainOutcomeReady { decision,
risk, candle, ... }`.

The 4-dimensional scoring lives inside the LLM prompt
(`src/llm/prompts.rs`):

```
TA  weight 40%
Sentiment 25%
Fundamental 20%
Risk 15%
```

### 3.6 `TraderManagerAgent` — `src/agents/manager.rs`

Second LLM with its own prompt (`MANAGER_SYSTEM_PROMPT`). Inputs:

- Brain proposal (entry, SL, TP, size, reasoning)
- Active lessons
- Latest feeds snapshot
- Latest `SurvivalState`

Outputs (strict JSON):

- `Approve`
- `Veto { reason }`
- `Adjust { size_mult ∈ [0.1, 1.5], sl_offset_bps ∈ [-50, 50], tp_offset_bps ∈ [-50, 50] }`

Special behaviours:

- **Survival hard-veto** — if `mode ∈ { Frozen, Dead }`, instantly emit
  `Veto` without an LLM call.
- **Fast approve** — when brain confidence ≥ `fast_approve_min_conf`
  AND no active lessons matched AND survival is `Healthy`, the LLM is
  skipped (token saver).
- **Fail closed** — any HTTP error / timeout → `Veto` (not Approve).

### 3.7 `ExecutionAgent` — `src/agents/execution.rs`

Owns the `Exchange` impl (`PaperExchange` or `BinanceFutures`) and the
`PositionBook`. On `ManagerVerdictEmitted::Approve|Adjust`:

1. Build the entry `OrderRequest` with deterministic
   `client_id = idempotent_client_id(symbol, strategy, side, entry, size)`.
2. Dispatch the entry. If filled, push two protective orders to the
   broker:
   - `STOP_MARKET` with `closePosition=true`, `reduce_only=true`,
     `stop_price = entry ± sl_offset`.
   - `TAKE_PROFIT_MARKET` with `closePosition=true`, `reduce_only=true`,
     `stop_price = entry ± tp_offset`.
3. Insert the open position into `PositionBook`.

Also handles:

- `ControlCommand::FlatAll` → close every open position at market.
- Survival mode `Frozen|Dead` → reject new entries immediately.
- `OrderFilled` / `PositionClosed` events for downstream agents.

### 3.8 `MonitorAgent` — `src/agents/monitor.rs`

Listens to every relevant event and:

- Updates `MetricsState` (the snapshot served at `/metrics`).
- Inserts every closed trade into the SQLite journal (`trades.db`)
  with full LLM reasoning (40+ columns).
- Sends Telegram alerts on order fills, position closes, manager
  vetoes, and survival mode transitions.

### 3.9 `LearningAgent` — `src/agents/learning.rs`

Background tokio task that wakes every 5 minutes, reads the latest 500
trades from the journal, rebuilds `PerformanceMemory`, derives
`Lesson`s, and atomically swaps the `LearningPolicy` snapshot. Emits
`PolicyRefreshed { lesson_count, ts }`.

### 3.10 `SurvivalAgent` — `src/agents/survival.rs`

The "stay alive" engine. Two independent tokio tasks:

1. **Equity refresh** (every `equity_refresh_secs`, default 60):
   - `exchange.fetch_equity_usd()` → `risk.set_equity(eq)`.
   - Emits `EquityReconciled { equity_usd, ts }`.

2. **State refresh** (every `refresh_secs`, default 15):
   - Reads risk snapshot + recent feed snapshots + recent trade outcomes.
   - Computes `survive_score` (see [docs/SURVIVAL.md](SURVIVAL.md)).
   - Determines `SurvivalMode` and `size_multiplier`.
   - Calls `risk.set_size_multiplier(...)` and (un)freezes the manager.
   - Broadcasts `SurvivalUpdated(state)`.
   - On death-line or extreme drawdown breach, broadcasts
     `ControlCommand::FlatAll { reason }` (debounced 60 s).

Every `PositionClosed` event also feeds into the cooldown tracker,
which can flip the mode to `Frozen` independently of equity.

### 3.11 `ControlAgent` — `src/agents/control.rs`

Three ingress paths:

1. **Telegram long-poll** (when enabled) — issues `getUpdates` every
   `poll_secs`, parses commands from `allowed_user_ids` only,
   produces a reply, and sends it back.
2. **`/tmp/aria.control` file** — monitors size; on change reads
   lines, parses `freeze` / `unfreeze` / `flat`, then truncates.
3. **`ControlCommand` events on the bus** — synced into the
   `RiskManager` (freeze/unfreeze) and `ExecutionAgent` (flat).

Available commands: `/status`, `/positions`, `/freeze`, `/unfreeze`,
`/flat`, `/health`, `/help`. See [docs/CONTROL.md](CONTROL.md).

### 3.12 `WatchdogAgent` — `src/agents/watchdog.rs`

Subscribes to `Heartbeat` events. Tracks last-seen timestamp per
agent. Every `check_interval_secs` (default 15):

- If any monitored agent is silent for more than `liveness_timeout_secs`
  (default 90), publishes `ControlCommand::Freeze { reason }`.
- When all agents are alive again, publishes `ControlCommand::Unfreeze`.

This is the dead-man-switch — if e.g. `DataAgent` is wedged on a stuck
WebSocket, the bot stops opening new trades automatically.

---

## 4. `AgentEvent` enum reference

```rust
// src/agents/messages.rs

pub enum AgentEvent {
    // From DataAgent
    Tick { symbol: String, trade: Trade },
    BookTicker { symbol, bid_px, bid_qty, ask_px, ask_qty, ts },
    CandleClosed { symbol: String, candle: Candle },

    // From FeedsAgent
    FeedsSnapshot(FeedsSnapshotMsg),

    // From SignalAgent
    PreSignalEmitted { pre_signal, regime, candle, ts },

    // From RiskAgent
    RiskVerdict(RiskVerdictMsg),

    // From BrainAgent
    BrainOutcomeReady(BrainOutcome),

    // From LearningAgent
    PolicyRefreshed { lesson_count, ts },

    // From TraderManagerAgent
    ManagerVerdictEmitted(ManagerVerdict),

    // From ExecutionAgent
    OrderFilled { ... },
    PositionClosed { ... },

    // From every agent
    Heartbeat { from: AgentId, ts: DateTime<Utc> },

    // From SurvivalAgent
    SurvivalUpdated(SurvivalState),
    EquityReconciled { equity_usd: f64, ts: DateTime<Utc> },

    // From ControlAgent / WatchdogAgent
    ControlCommand(ControlCommand),
    // ControlCommand: Freeze { reason } | Unfreeze | FlatAll { reason }
    //               | ResetDaily | StatusRequest

    // Lifecycle
    Shutdown,
}
```

`AgentId` enumerates every producer:

```
Data, Feeds, Signal, Risk, Brain, Learning, Manager, Execution,
Monitor, Survival, Control
```

(There is no `Watchdog` AgentId because it does not emit heartbeats —
only `Freeze`/`Unfreeze`.)

---

## 5. Message flow — happy path

```
DataAgent: CandleClosed(BTCUSDT, candle)
  └─> SignalAgent: PreSignalEmitted(BTCUSDT, mean_reversion, BUY, conf 78)
        └─> RiskAgent: RiskVerdict(allowed, size 0.012)
              └─> BrainAgent: BrainOutcomeReady(GO, conf 82, "trend confirmation high probability")
                    └─> TraderManagerAgent: ManagerVerdictEmitted(Approve)
                          └─> ExecutionAgent:
                                1. OrderRequest(client_id=det:42d3c1b8, MARKET, BUY 0.012)
                                2. order filled → OrderFilled
                                3. SL: STOP_MARKET reduce_only=true closePosition=true stop_price=66800
                                4. TP: TAKE_PROFIT_MARKET reduce_only=true closePosition=true stop_price=68100
                                5. PositionBook.insert(...)
                          └─> MonitorAgent: journal + Telegram alert
```

---

## 6. Quant research and microstructure modules

The library now includes reusable quant primitives for the roadmap items
from `prompt-1777632664168.md`:

- `src/research/` — IC/IR tracking, IC decay curves, walk-forward splits,
  and permutation p-values for signal significance checks.
- `src/microstructure/` — OFI, VPIN, and toxicity helpers for filtering
  or confirming entries during adverse order-flow regimes.
- `src/portfolio/` — capped Kelly sizing, volatility targeting,
  return correlation, exposure caps, historical VaR, and CVaR utilities.
- `src/execution/quality.rs` — implementation shortfall decomposition
  into delay cost and market impact.
- `src/execution/limit_order.rs` — join/cross/post-only planning plus
  deterministic fill-probability estimates.
- `src/strategy/multi_timeframe.rs` — weighted multi-timeframe vote
  aggregation for higher-timeframe confirmation.
- `src/backtest/monte_carlo.rs` — drawdown confidence intervals from
  deterministic PnL reshuffles.
- `src/strategy/hmm.rs` and `src/strategy/kalman.rs` — probabilistic
  regime inference and trend estimation primitives.
- `src/strategy/pairs.rs` and `src/feeds/funding_arb.rs` — pairs-trading
  spread helpers and funding-edge classification.
- `src/feeds/alt_data.rs` and `src/feeds/options.rs` — normalized
  alternative-data scoring and public Deribit BTC/ETH options-skew
  snapshots.
- `[advanced_alpha]` can wire external-data/funding/Kalman context into
  `SignalAgent` as a pre-risk confirmation gate. It is disabled by
  default and never directly sizes orders.
- Backtest mode emits a compact research report table by default; set
  `ARIA_RESEARCH_REPORT_FORMAT=json` for JSON output.

These modules are intentionally independent and test-covered so they can
be wired deeper into live sizing/strategy selection in small, auditable PRs.

---

## 7. Message flow — fail-closed scenarios

### 7.1 Manager LLM down

```
TraderManagerAgent:
  call_manager_llm(...) -> Err(timeout)
  -> warn!("manager LLM failed — failing CLOSED with Veto")
  -> emit ManagerVerdictEmitted(Veto { reason: "manager LLM unavailable" })

ExecutionAgent: sees Veto, no order dispatched.
```

### 7.2 Survival mode = Frozen

```
TraderManagerAgent:
  if survival.mode in {Frozen, Dead}:
    -> emit ManagerVerdictEmitted(Veto { reason: "survival mode Frozen (score 18)" })
       (no LLM call made)
```

### 7.3 Death line breach

```
SurvivalAgent (every 15s):
  state.equity = 3499; death_line = 3500
  -> mode = Dead
  -> emit SurvivalUpdated(...)
  -> emit ControlCommand::FlatAll { reason: "death line breached" }

ExecutionAgent: closes every open position at market.
RiskAgent: subsequent verdicts now blocked by survival hard-gate.
```

### 7.4 Watchdog trip

```
DataAgent stops emitting heartbeats for 91 seconds.

WatchdogAgent (every 15s):
  -> last_seen[Data] is too old
  -> emit ControlCommand::Freeze { reason: "Data agent silent for 91s" }

ControlAgent loop receives Freeze -> risk.freeze("Data agent silent...")
RiskAgent: every subsequent verdict is Blocked.

Later, DataAgent reconnects and starts emitting heartbeats again:
WatchdogAgent: -> emit ControlCommand::Unfreeze
ControlAgent: -> risk.unfreeze()
```

---

## 7. Concurrency model

- Every agent runs in `tokio::spawn(...)`. There are no `std::thread`s.
- The bus is `tokio::sync::broadcast::channel(4096)`. Each agent
  `subscribe()`s and gets its own `Receiver`. Slow consumers lag
  rather than block producers (the `Receiver::recv()` returns `Lagged`
  which is logged but not fatal).
- Only `RiskManager` and `PositionBook` are shared via `Arc<...>`.
  Everything else is task-local.
- `RiskManager` is internally `Arc<Mutex<Inner>>` — every method takes
  the mutex briefly. This is fine because risk decisions are sub-millisecond.
- `SurvivalState` is published as a *value* (cloned per receiver),
  not a shared reference. Each agent caches its own latest copy.

This makes ARIA fundamentally crash-isolated: a panic in one agent
brings down only that agent's task, and (with `Restart=on-failure`
under systemd) the whole binary restarts cleanly with broker-side
SL/TP still in place.

---

## 8. Where to start when reading the code

Recommended reading order:

1. `src/agents/messages.rs` — the `AgentEvent` enum is the contract.
2. `src/agents/bus.rs` — trivial wrapper around `broadcast::channel`.
3. `src/main.rs` `run_agents()` — see how all agents are wired.
4. `src/agents/survival.rs` — the soul of "trade for life".
5. `src/agents/manager.rs` — how the second-LLM verdict layer works.
6. `src/agents/execution.rs` — broker-side SL/TP and idempotent IDs.
7. `src/llm/prompts.rs` — what the LLM actually sees.
