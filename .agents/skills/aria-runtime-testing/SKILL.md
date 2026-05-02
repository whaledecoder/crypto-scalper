# ARIA Runtime Testing

Use this skill when validating the crypto-scalper bot runtime after changes to signal, risk, brain, manager, execution, or configuration paths.

## Setup

- Repo path: `/home/ubuntu/repos/crypto-scalper`
- The bot loads `.env` automatically from the repo root when run from the repo.
- For local visibility, use `ARIA_CONFIG_OVERLAY=config/aggressive.toml`; the overlay keeps `[mode] dry_run = true` and `run_mode = "paper"`.
- Never commit `.env` or other secret files.

## Static validation

Run these before opening or updating a PR:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib
cargo build --release
```

## Runtime validation

Run the paper bot from the repo root:

```bash
ARIA_MANAGER_ENABLED=true RUST_LOG=info cargo run --release
```

Expected healthy paper-mode progression after a few 1m candles with `config/aggressive.toml`:

1. Bootstrap completes for all configured symbols, possibly after Binance Vision fallback.
2. Status shows nonzero `signals`.
3. At least one `risk: allowed signal` appears when a signal passes risk gates.
4. Brain logs/monitor status show `brain_go > 0`; if the LLM times out, TA-only fallback may produce `Go` when confidence meets `fallback_ta_threshold`.
5. Manager status increments and approves in paper mode when configured to fail open.
6. Status eventually shows `fills > 0` and no survival death-line freeze.

Useful status fields in logs: `signals`, `risk_allowed`, `risk_blocked`, `brain_go`, `manager`, `vetoes`, `fills`, `last_signal`, `last_block`, `last_brain`, `last_manager`.

If fills remain zero, inspect `last_block` first; common blockers are reward/risk, net edge after costs, TA threshold, LLM fallback threshold, or manager veto/timeouts.
