# Survival Mode — "Trade for Life"

Survival mode is the agent layer that exists for one job: **keep the bot
alive long enough to keep trading**. ARIA is autonomous; if the operator
isn't watching and the account hits the death line, the bot has to be
the one that shuts itself off. Every other layer of the stack consults
the `SurvivalState` before doing anything risky.

> Default bias is **conservative**. The death line is at **70 % of
> initial equity**. The manager LLM defaults to **Veto** on any error.
> Cooldowns are **automatic**.

---

## 1. The `SurvivalState`

Published every `[survival] refresh_secs` (default 15 s) by the
`SurvivalAgent`:

```rust
pub struct SurvivalState {
    pub score: u8,                 // 0..=100
    pub mode: SurvivalMode,
    pub equity_usd: f64,
    pub initial_equity_usd: f64,
    pub death_line_usd: f64,       // = initial * death_line_pct
    pub peak_equity_usd: f64,
    pub realized_pnl_today: f64,
    pub realized_pnl_pct_today: f64,
    pub drawdown_pct: f64,         // from peak
    pub open_positions: u32,
    pub consecutive_losses: u32,
    pub last_loss_at: Option<DateTime<Utc>>,
    pub size_multiplier: f64,
    pub reasons: Vec<String>,      // human-readable explanation
    pub ts: DateTime<Utc>,
}

pub enum SurvivalMode {
    Healthy,    // size ×1.0
    Cautious,   // size ×0.6
    Defensive,  // size ×0.3
    Frozen,     // size ×0.0 — refuses entries
    Dead,       // size ×0.0 — auto-flat + permanent freeze
}
```

Live snapshot:

```bash
curl -s http://localhost:9184/survival | jq .
```

---

## 2. `survive_score` formula

The score starts at 100 and accumulates penalties based on the current
state of the world. The exact constants live in
`src/agents/survival.rs`; below is the canonical version.

```
score = 100

# 2.1 Drawdown penalty (max 60)
score -= min(60, drawdown_pct / auto_flat_drawdown_pct * 60)

# 2.2 Daily loss penalty (max 40)
if realized_pnl_today < 0:
    score -= min(40, abs(realized_pnl_pct_today) * 6)

# 2.3 Loss-streak penalty (max 30)
if consecutive_losses >= 1:
    score -= min(30, (consecutive_losses - 1) * 8)

# 2.4 Cooldown override
if cooldown_active:
    score = min(score, 20)        # forces Frozen

# 2.5 News penalty
if news_score < news_panic_threshold:        # default -0.6
    score -= 25
elif news_score > news_euphoria_threshold:   # default +0.8
    score -= 10

# 2.6 Equity-floor penalty
buffer = (equity - death_line) / (initial - death_line)    # 0..1
if buffer <= 0.05:    score -= 30
elif buffer <= 0.10:  score -= 15
if equity <= death_line:   score = 0   # forces Dead

# 2.7 Daily ratchet
if realized_pnl_pct_today >= daily_pnl_ratchet_pct  AND
   realized_pnl_today < (peak_realized_today * 0.5):
    score = 0   # forces Frozen — gain wiped half-back, lock the rest

score = clamp(score, 0, 100)
```

The thresholds that translate `score` into `mode`:

| Condition | Mode | Size mult |
|---|---|---|
| `equity ≤ death_line` | **Dead** | 0.0 |
| `cooldown_active` OR `ratchet_locked` OR `score < 25` | **Frozen** | 0.0 |
| `score < 50` | **Defensive** | 0.3 |
| `score < 80` | **Cautious** | 0.6 |
| else | **Healthy** | 1.0 |

The `reasons` field contains the list of penalties that fired this
cycle, e.g.:

```json
"reasons": [
    "drawdown 4.2% (penalty 31)",
    "consecutive losses 3 (penalty 16)",
    "news score -0.71 below panic threshold (penalty 25)"
]
```

---

## 3. Cooldown windows

Every closed position is fed into the cooldown tracker:

```rust
fn on_position_closed(&mut self, pnl: f64, symbol: &str, strategy: &str) { … }
```

Three independent windows can each force a `Frozen` mode:

| Trigger | Window | Cooldown |
|---|---|---|
| `[survival] loss_streak_short` consecutive losses *(default 3)* | none | `loss_streak_short_cooldown_min` *(default 30 min)* |
| `[survival] loss_streak_long` losses within `loss_streak_long_window_min` *(default 5 in 60 min)* | rolling | `loss_streak_long_cooldown_min` *(default 240 min)* |
| `[survival] daily_loss_count` losses in a single day *(default 10)* | calendar day | until next UTC midnight |

A single winning trade resets the *consecutive* counter — but the
rolling window and the daily count keep accumulating.

---

## 4. Death line + drawdown auto-flat

```
death_line = risk.equity_usd * survival.death_line_pct        # default 0.70
auto_flat  = peak_equity * (1 - auto_flat_drawdown_pct/100)   # default 8 %
```

Two emergency paths:

1. **Equity ≤ death_line** → `mode = Dead`:
   - `SurvivalAgent` broadcasts `ControlCommand::FlatAll { reason: "death line breached" }`.
   - `ExecutionAgent` closes every position at market.
   - `RiskManager` is `freeze()`d permanently; only an operator
     `/unfreeze` (Telegram or `/tmp/aria.control`) can resume.
2. **Equity ≤ auto_flat** → flat-all, but the freeze auto-clears
   when the score recovers past 50 (mode becomes `Defensive`/`Cautious`).

The flat-all broadcast is **debounced for 60 seconds** so a flapping
state doesn't spam orders.

---

## 5. Daily PnL ratchet

```
ratchet_pct = [survival] daily_pnl_ratchet_pct      # default 2 %
```

When today's realised PnL crosses `+2 %`, ARIA records the peak. If
the bot then gives back more than half of that gain, `score = 0` and
the mode flips to `Frozen`. The lock auto-releases once the realised
PnL climbs back above the recorded peak — i.e. the bot has earned the
right to keep playing.

The intent is "lock in the day". A profitable session shouldn't be
allowed to round-trip into a loss because the bot got greedy late.

---

## 6. News blackout

`SurvivalAgent` reads the latest aggregated news score from `FeedsAgent`:

| `news_score` | Action |
|---|---|
| `< news_panic_threshold` *(default −0.6)* | `score -= 25`, can flip mode to `Frozen` for 2 hours |
| `< 0` to `> -0.6` | no effect |
| `> news_euphoria_threshold` *(default +0.8)* | `score -= 10` (avoid FOMO, halve size via Defensive/Cautious) |

The 2-hour timer is reset every time a fresh panic snapshot lands, so
a sustained news event keeps the bot frozen until conditions improve.

---

## 7. Tiered risk

The `size_multiplier` propagates to `RiskManager.calculate_size()`:

```rust
let risk_amount = i.equity * i.limits.risk_per_trade_pct / 100.0
                  * i.size_multiplier;     // ← survival multiplier
```

So a `Cautious` mode automatically reduces every entry to 60 % of the
configured risk per trade, and `Defensive` reduces it to 30 %. The
manager LLM **also** sees this multiplier in its prompt and may
further reduce via an `Adjust` verdict — multipliers stack.

| Mode | Multiplier |
|---|---|
| Healthy | 1.0 |
| Cautious | 0.6 |
| Defensive | 0.3 |
| Frozen | 0.0 (entry refused) |
| Dead | 0.0 (entry refused, auto-flat broadcast) |

---

## 8. Equity reconciliation

Independent task inside `SurvivalAgent`, runs every
`[survival] equity_refresh_secs` (default 60 s):

```rust
match exchange.fetch_equity_usd().await {
    Ok(eq) if eq > 0.0 => {
        risk.set_equity(eq);
        bus.publish(AgentEvent::EquityReconciled { equity_usd: eq, ts: now });
    }
    _ => warn!("equity reconciliation failed"),
}
```

This is what guarantees the in-memory `RiskManager.equity()` matches
broker truth. Without this, sizing drifts if the manual deposits or
withdrawals happen, or if the bot's PnL accounting disagrees with
Binance's.

---

## 9. Tuning playbook

| Symptom | Likely fix |
|---|---|
| Bot freezes too often after small drawdowns | Lower `[survival] auto_flat_drawdown_pct` cautiously, or raise `risk.max_drawdown_pct` |
| Bot keeps trading through obviously bad streaks | Lower `[survival] loss_streak_short` or shorten `loss_streak_long_window_min` |
| Death line triggered too easily on ranging markets | Raise `death_line_pct` to e.g. 0.60 (allow 40 % drawdown before death) — only do this if you're sure |
| News panic vetoes too aggressive | Set `news_panic_threshold` closer to -0.8 |
| Daily ratchet locks profits prematurely | Raise `daily_pnl_ratchet_pct` from 2 % to e.g. 5 % |

Always test changes in **paper mode** for at least a day before going live.

---

## 10. Related files

- `src/agents/survival.rs` — the agent itself
- `src/agents/messages.rs` — `SurvivalState`, `SurvivalMode`,
  `ControlCommand`
- `src/config.rs` — `SurvivalCfg` defaults (`fn default_*`)
- `src/execution/risk.rs` — `RiskManager.set_size_multiplier`,
  `freeze`, `unfreeze`, `is_frozen`, `is_blocked`
- `src/agents/manager.rs` — survival hard-veto in the manager LLM path
