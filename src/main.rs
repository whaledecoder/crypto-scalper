//! ARIA — Autonomous Realtime Intelligence Analyst
//!
//! Top-level binary. Loads config and starts the multi-agent runtime:
//! every layer of the stack runs as an independent tokio task that
//! communicates exclusively over a typed `MessageBus`. The
//! `TraderManagerAgent` (when enabled) sits between the brain and the
//! exchange and gives the final approve / veto / adjust verdict.

use anyhow::{Context, Result};
use crypto_scalper::{
    agents::messages::ControlCommand,
    agents::{
        bus::MessageBus, control::ControlAgentDeps, execution::ExecutionAgentDeps,
        manager::ManagerAgentConfig, messages::AgentEvent, risk::RiskAgentConfig,
        survival::SurvivalAgentDeps, watchdog::WatchdogConfig,
    },
    backtest::{load_csv, BacktestEngine},
    config::Config,
    data::Timeframe,
    execution::{
        binance::BinanceFutures, position::Position, Exchange, PaperExchange, PositionBook,
        RiskManager,
    },
    execution::{risk::RiskLimits, tcm::TransactionCostModel},
    feeds::{
        DeribitOptionsClient, ExternalSnapshot, FearGreedClient, FundingClient, NewsClient,
        OnchainClient, SentimentClient,
    },
    learning::{lessons::LessonConfig, LearningPolicy},
    llm::engine::{LlmEngine, LlmEngineConfig, LlmProvider},
    monitoring::{
        logger::TradeJournal, spawn_dashboard_server, DashboardState, MetricsState,
        TelegramNotifier,
    },
    quant::{QuantConfig, QuantEngine},
    research::{reports_to_json, reports_to_markdown, ResearchReport},
    strategy::state::{StrategyName, SymbolState},
};
use parking_lot::RwLock as PlRwLock;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Load `.env` (if any) before tracing so RUST_LOG works too.
    load_dotenv();
    init_tracing();

    let default_path = PathBuf::from("config/default.toml");
    let overlay_path = overlay_path_from_env();
    let cfg = Config::load(&default_path, overlay_path.as_deref())
        .context("failed to load configuration")?;

    info!(mode = %cfg.mode.run_mode, dry_run = cfg.mode.dry_run, "starting ARIA");

    match cfg.mode.run_mode.as_str() {
        "backtest" => run_backtest(&cfg).await,
        _ => run_agents(cfg).await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn overlay_path_from_env() -> Option<PathBuf> {
    std::env::var("ARIA_CONFIG_OVERLAY").ok().map(PathBuf::from)
}

/// Load environment variables from a `.env` file, if present.
///
/// Search order:
/// 1. `ARIA_DOTENV` env var (explicit path)
/// 2. `./.env` (current working directory)
/// 3. The directory containing the binary (handy for symlinked `aria`)
///
/// Lines like `KEY=VALUE` (optionally `export KEY=VALUE`) are parsed.
/// Quoted values (`"..."` or `'...'`) get the quotes stripped. Any
/// variable already present in the process environment is preserved
/// (so a real export still wins over the file).
fn load_dotenv() {
    let candidates: Vec<PathBuf> = {
        let mut v = Vec::new();
        if let Ok(p) = std::env::var("ARIA_DOTENV") {
            v.push(PathBuf::from(p));
        }
        v.push(PathBuf::from(".env"));
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                v.push(dir.join(".env"));
            }
        }
        v
    };

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let mut val = value.trim().to_string();
            // Strip surrounding quotes if balanced.
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = val[1..val.len() - 1].to_string();
            }
            // Don't overwrite a real export already in the env.
            if std::env::var(key).is_err() {
                std::env::set_var(key, val);
            }
        }
        // Stop at the first .env we successfully parse.
        eprintln!("loaded env from {}", path.display());
        break;
    }
}

async fn run_backtest(cfg: &Config) -> Result<()> {
    let data_dir = PathBuf::from(&cfg.backtest.data_dir);
    if !data_dir.exists() {
        anyhow::bail!("backtest data dir not found: {}", data_dir.display());
    }

    let active: Vec<StrategyName> = cfg
        .strategy
        .active
        .iter()
        .filter_map(|s| StrategyName::parse(s))
        .collect();
    let interval_secs = cfg
        .pairs
        .timeframes
        .first()
        .and_then(|s| Timeframe::parse(s).ok())
        .map(|t| t.seconds)
        .unwrap_or(300);

    let mut reports = Vec::new();
    for symbol in &cfg.pairs.symbols {
        let file = data_dir.join(format!("{symbol}.csv"));
        if !file.exists() {
            warn!(csv = %file.display(), "missing backtest csv — skipping");
            continue;
        }
        let candles = load_csv(&file, interval_secs)?;
        let engine = BacktestEngine {
            symbol: symbol.clone(),
            active: active.clone(),
            min_ta_confidence: cfg.strategy.min_ta_confidence,
            risk_per_trade_usd: cfg.risk.equity_usd * cfg.risk.risk_per_trade_pct / 100.0,
            fee_bps: cfg.backtest.fee_bps,
            slippage_bps: cfg.backtest.slippage_bps,
            market_impact_bps: cfg.backtest.market_impact_bps,
            min_reward_risk: cfg.risk.min_reward_risk,
            max_position_notional_pct: cfg.risk.max_position_notional_pct,
            min_net_edge_bps: cfg.risk.min_net_edge_bps,
            assumed_daily_volume_usd: cfg.risk.assumed_daily_volume_usd,
            equity_usd: cfg.risk.equity_usd,
            trading_days_per_year: cfg.backtest.trading_days_per_year,
            trades_per_day: cfg.backtest.trades_per_day,
        };
        let result = engine.run(&candles)?;
        reports.push(ResearchReport::from_backtest(&result));
        info!(
            symbol = %symbol,
            trades = result.trades.len(),
            win_rate = %format!("{:.2}%", result.metrics.win_rate * 100.0),
            pf = %format!("{:.2}", result.metrics.profit_factor),
            net = %format!("{:.2}", result.metrics.net_pnl),
            "backtest symbol done"
        );
    }
    if !reports.is_empty() {
        let format =
            std::env::var("ARIA_RESEARCH_REPORT_FORMAT").unwrap_or_else(|_| "markdown".into());
        match format.as_str() {
            "json" => println!("{}", reports_to_json(&reports)),
            _ => println!("{}", reports_to_markdown(&reports)),
        }
    }
    Ok(())
}

/// Spawn the full multi-agent runtime: data, feeds, signal, risk,
/// brain, manager, execution, monitor, and the periodic learning
/// refresh agent. Exits cleanly on Ctrl-C by broadcasting `Shutdown`.
async fn run_agents(cfg: Config) -> Result<()> {
    let bus = MessageBus::new(4096);

    // --- Exchange ---
    let exchange: Arc<dyn Exchange> = if cfg.mode.run_mode == "live" && !cfg.mode.dry_run {
        info!("live mode — dispatching real orders to Binance");
        Arc::new(BinanceFutures::new(
            cfg.exchange.rest_base_url.clone(),
            cfg.exchange.api_key.clone(),
            cfg.exchange.api_secret.clone(),
            cfg.exchange.recv_window_ms,
        ))
    } else {
        info!("paper/dry-run mode — simulated exchange");
        Arc::new(PaperExchange::new(2.0, cfg.risk.equity_usd))
    };

    // --- Risk manager (shared between RiskAgent + ExecutionAgent) ---
    let risk = Arc::new(RiskManager::new(
        RiskLimits {
            risk_per_trade_pct: cfg.risk.risk_per_trade_pct,
            max_open_positions: cfg.risk.max_open_positions,
            max_daily_loss_pct: cfg.risk.max_daily_loss_pct,
            max_drawdown_pct: cfg.risk.max_drawdown_pct,
            max_leverage: cfg.risk.max_leverage,
            max_spread_pct: cfg.risk.max_spread_pct,
            min_reward_risk: cfg.risk.min_reward_risk,
            max_position_notional_pct: cfg.risk.max_position_notional_pct,
            min_net_edge_bps: cfg.risk.min_net_edge_bps,
            assumed_daily_volume_usd: cfg.risk.assumed_daily_volume_usd,
        },
        cfg.risk.equity_usd,
    ));

    let book = Arc::new(PositionBook::new());
    let journal = Arc::new(TradeJournal::open(&cfg.monitoring.db_path)?);
    let telegram = Arc::new(TelegramNotifier::new(
        cfg.monitoring.telegram_bot_token.clone(),
        std::env::var("TELEGRAM_CHAT_ID")
            .unwrap_or_else(|_| cfg.monitoring.telegram_chat_id.clone()),
    ));

    let metrics = MetricsState::new(&cfg.mode.run_mode);
    let bind = cfg
        .monitoring
        .metrics_bind
        .parse::<std::net::SocketAddr>()
        .context("invalid metrics bind")?;

    // --- Brain LLM ---
    let provider = LlmProvider::parse(&cfg.llm.provider);
    info!(
        provider = %cfg.llm.provider,
        model = %cfg.llm.model,
        api_base = %cfg.llm.api_base,
        key_set = !cfg.llm.api_key.is_empty(),
        "brain llm configured"
    );
    let llm = Arc::new(LlmEngine::new(LlmEngineConfig {
        provider,
        api_key: cfg.llm.api_key.clone(),
        api_base: cfg.llm.api_base.clone(),
        model: cfg.llm.model.clone(),
        timeout_secs: cfg.llm.timeout_secs,
        max_tokens: cfg.llm.max_tokens,
        fallback_ta_threshold: cfg.llm.fallback_ta_threshold,
        http_referer: Some(cfg.llm.http_referer.clone()).filter(|s| !s.is_empty()),
        http_app_title: Some(cfg.llm.http_app_title.clone()).filter(|s| !s.is_empty()),
    }));

    // --- Feeds ---
    let fear_greed = Arc::new(FearGreedClient::new());
    let funding = Arc::new(FundingClient::new(cfg.exchange.rest_base_url.clone()));
    let news = Arc::new(NewsClient::new(
        Some(cfg.feeds.cryptopanic_api_key.clone()).filter(|s| !s.is_empty()),
        cfg.feeds.rss_feeds.clone(),
    ));
    let sentiment = Arc::new(SentimentClient::new(
        Some(cfg.feeds.lunarcrush_api_key.clone()).filter(|s| !s.is_empty()),
    ));
    let onchain = Arc::new(OnchainClient::new(
        Some(cfg.feeds.glassnode_api_key.clone()).filter(|s| !s.is_empty()),
        Some(cfg.feeds.whalealert_api_key.clone()).filter(|s| !s.is_empty()),
    ));
    let options = Arc::new(DeribitOptionsClient::new(
        cfg.feeds.deribit_base_url.clone(),
    ));

    // --- Per-symbol state (owned by SignalAgent, read by BrainAgent) ---
    let interval_secs = cfg
        .pairs
        .timeframes
        .first()
        .and_then(|s| Timeframe::parse(s).ok())
        .map(|t| t.seconds)
        .unwrap_or(300);
    let mut states_map: HashMap<String, SymbolState> = HashMap::new();
    for s in &cfg.pairs.symbols {
        states_map.insert(s.clone(), SymbolState::new(s));
    }
    let states = Arc::new(Mutex::new(states_map));

    // --- Bootstrap historical candles so indicators are warm on day-1 ---
    // Without this, EMA200 needs 200 live candles (= 16h+ at 5m) before
    // EmaRibbon can fire, and ADX needs ~28 candles before RegimeDetector
    // can classify anything other than Unknown.
    {
        let bootstrap_tf = cfg
            .pairs
            .timeframes
            .first()
            .and_then(|s| Timeframe::parse(s).ok())
            .unwrap_or(Timeframe { seconds: 300 });
        crypto_scalper::data::bootstrap_states(&states, &cfg.exchange.rest_base_url, &bootstrap_tf)
            .await;
    }

    let active: Vec<StrategyName> = cfg
        .strategy
        .active
        .iter()
        .filter_map(|s| StrategyName::parse(s))
        .collect();

    let policy = LearningPolicy::default();
    let feeds_cache: Arc<PlRwLock<HashMap<String, ExternalSnapshot>>> =
        Arc::new(PlRwLock::new(HashMap::new()));
    let survival_state: Arc<PlRwLock<Option<crypto_scalper::agents::SurvivalState>>> =
        Arc::new(PlRwLock::new(None));

    // --- Dashboard server ---
    let _metrics_handle = spawn_dashboard_server(
        DashboardState {
            metrics: Arc::clone(&metrics),
            policy: Some(policy.clone()),
            survival: Arc::clone(&survival_state),
        },
        bind,
    );

    // Forward SurvivalUpdated events to the dashboard's snapshot.
    {
        let bus_sub = bus.clone();
        let survival_state = Arc::clone(&survival_state);
        tokio::spawn(async move {
            let mut rx = bus_sub.subscribe();
            while let Ok(ev) = rx.recv().await {
                match ev {
                    crypto_scalper::agents::messages::AgentEvent::SurvivalUpdated(s) => {
                        *survival_state.write() = Some(s);
                    }
                    crypto_scalper::agents::messages::AgentEvent::Shutdown => break,
                    _ => {}
                }
            }
        });
    }

    // --- Reconcile open positions from exchange on startup ---
    // If the bot crashed mid-trade, the PositionBook is empty but the exchange
    // may still hold open positions. Fetch them and re-seed the book so SL/TP
    // exit checks and risk counters stay consistent.
    if cfg.mode.run_mode == "live" {
        match exchange.fetch_open_positions(&cfg.pairs.symbols).await {
            Ok(snaps) if !snaps.is_empty() => {
                let mut recon: Vec<Position> = Vec::new();
                for s in &snaps {
                    let pos = Position {
                        client_id: format!("recon-{}-{}", s.symbol, s.side.as_str()),
                        symbol: s.symbol.clone(),
                        side: s.side,
                        size: s.size,
                        entry_price: s.entry_price,
                        stop_loss: 0.0, // unknown — broker holds protective orders
                        take_profit: 0.0,
                        opened_at: chrono::Utc::now(),
                        trailing_activated: false,
                        peak_price: s.mark_price,
                        trough_price: s.mark_price,
                    };
                    recon.push(pos);
                    risk.on_position_opened();
                }
                let n = recon.len();
                book.reconcile(recon);
                warn!(
                    count = n,
                    "startup: reconciled open positions from exchange"
                );
            }
            Ok(_) => info!("startup: no open positions to reconcile"),
            Err(e) => warn!(error = %e, "startup: position reconciliation failed"),
        }
    }

    // --- Spawn agents ---
    let _data = crypto_scalper::agents::data::spawn(
        bus.clone(),
        crypto_scalper::agents::data::DataAgentConfig {
            ws_base_url: cfg.exchange.ws_base_url.clone(),
            symbols: cfg.pairs.symbols.clone(),
            interval_secs,
        },
    );
    let _feeds = crypto_scalper::agents::feeds::spawn(
        bus.clone(),
        crypto_scalper::agents::feeds::FeedsAgentDeps {
            fear_greed,
            funding,
            news,
            sentiment,
            onchain,
            options,
        },
        cfg.pairs.symbols.clone(),
        60,
    );
    // --- Quant Engine (Kelly, vol-target, VaR, IC, Kalman) ---
    let quant_engine = Arc::new(QuantEngine::new(QuantConfig {
        enabled: cfg.quant.enabled,
        kelly_cap: cfg.quant.kelly_cap,
        kelly_min_trades: cfg.quant.kelly_min_trades,
        target_vol_annual: cfg.quant.target_vol_annual,
        max_vol_multiplier: cfg.quant.max_vol_multiplier,
        vol_window: cfg.quant.vol_window,
        var_confidence: cfg.quant.var_confidence,
        max_var_pct: cfg.quant.max_var_pct,
        ic_window: cfg.quant.ic_window,
        ic_min_abs: cfg.quant.ic_min_abs,
        ic_max_boost: cfg.quant.ic_max_boost,
        kalman_process_noise: cfg.quant.kalman_process_noise,
        kalman_measurement_noise: cfg.quant.kalman_measurement_noise,
        kalman_min_velocity_bps: cfg.quant.kalman_min_velocity_bps,
    }));

    let _signal = crypto_scalper::agents::signal::spawn(
        bus.clone(),
        Arc::clone(&states),
        active.clone(),
        cfg.schedule.clone(),
        cfg.advanced_alpha.clone(),
        Some(Arc::clone(&quant_engine)),
    );

    let _risk = crypto_scalper::agents::risk::spawn(
        bus.clone(),
        Arc::clone(&risk),
        policy.clone(),
        RiskAgentConfig {
            base_min_ta_threshold: cfg.strategy.min_ta_confidence,
            base_min_llm_floor: cfg.llm.min_confidence,
            tcm: TransactionCostModel {
                taker_fee_bps: cfg.backtest.fee_bps,
                maker_fee_bps: -1.0,
                avg_slippage_bps: cfg.backtest.slippage_bps,
                market_impact_bps: cfg.backtest.market_impact_bps,
            },
            ..RiskAgentConfig::default()
        },
        Some(Arc::clone(&quant_engine)),
    );
    let _brain = crypto_scalper::agents::brain::spawn(
        bus.clone(),
        Arc::clone(&llm),
        Arc::clone(&states),
        policy.clone(),
        Arc::clone(&feeds_cache),
    );
    let _manager = crypto_scalper::agents::manager::spawn(
        bus.clone(),
        ManagerAgentConfig {
            enabled: cfg.manager.enabled,
            provider: cfg.manager.provider.clone(),
            api_base: cfg.manager.api_base.clone(),
            api_key: cfg.manager.api_key.clone(),
            model: cfg.manager.model.clone(),
            timeout_secs: cfg.manager.timeout_secs,
            max_tokens: cfg.manager.max_tokens,
            http_referer: Some(cfg.manager.http_referer.clone()).filter(|s| !s.is_empty()),
            http_app_title: Some(cfg.manager.http_app_title.clone()).filter(|s| !s.is_empty()),
            fast_approve_min_conf: cfg.manager.fast_approve_min_conf,
            fail_closed_without_llm: cfg.mode.run_mode == "live" && !cfg.mode.dry_run,
            fail_open_on_error: cfg.manager.fail_open_on_error,
        },
        policy.clone(),
        Arc::clone(&feeds_cache),
    );
    // --- Reconcile broker truth at startup (A3 + A4) ---
    if cfg.mode.run_mode == "live" && !cfg.mode.dry_run {
        for sym in &cfg.pairs.symbols {
            if let Err(e) = exchange
                .set_leverage(sym, cfg.risk.max_leverage as u8)
                .await
            {
                warn!(symbol = %sym, error = %e, "set_leverage failed");
            }
        }
        match exchange.fetch_equity_usd().await {
            Ok(eq) if eq > 0.0 => {
                info!(equity = eq, "startup: equity reconciled");
                risk.set_equity(eq);
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "startup: fetch_equity_usd failed"),
        }
        match exchange.fetch_open_positions(&cfg.pairs.symbols).await {
            Ok(positions) => {
                let now = chrono::Utc::now();
                let recovered: Vec<Position> = positions
                    .into_iter()
                    .map(|p| Position {
                        client_id: format!("recovered-{}-{}", p.symbol, now.timestamp_millis()),
                        symbol: p.symbol,
                        side: p.side,
                        size: p.size.abs(),
                        entry_price: p.entry_price,
                        stop_loss: 0.0,
                        take_profit: 0.0,
                        opened_at: now,
                        trailing_activated: false,
                        peak_price: p.mark_price,
                        trough_price: p.mark_price,
                    })
                    .collect();
                if !recovered.is_empty() {
                    info!(
                        count = recovered.len(),
                        "startup: reconciled open positions"
                    );
                }
                book.reconcile(recovered);
            }
            Err(e) => warn!(error = %e, "startup: fetch_open_positions failed"),
        }
    }

    let _execution = crypto_scalper::agents::execution::spawn(ExecutionAgentDeps {
        bus: bus.clone(),
        exchange: exchange.clone(),
        risk: Arc::clone(&risk),
        book: Arc::clone(&book),
        honor_survival: cfg.survival.enabled,
    });
    let _monitor = crypto_scalper::agents::monitor::spawn(
        bus.clone(),
        Arc::clone(&metrics),
        Arc::clone(&journal),
        Arc::clone(&telegram),
    );
    let _learning = crypto_scalper::agents::learning::spawn(
        bus.clone(),
        Arc::clone(&journal),
        policy.clone(),
        LessonConfig {
            equity_for_drawdown: cfg.risk.equity_usd,
            ..LessonConfig::default()
        },
        300,
        Some(Arc::clone(&quant_engine)),
    );

    let _survival = crypto_scalper::agents::survival::spawn(SurvivalAgentDeps {
        bus: bus.clone(),
        cfg: cfg.survival.clone(),
        exchange: exchange.clone(),
        risk: Arc::clone(&risk),
        initial_equity: cfg.risk.equity_usd,
    });

    let _control = crypto_scalper::agents::control::spawn(ControlAgentDeps {
        bus: bus.clone(),
        cfg: cfg.control.clone(),
        telegram_token: cfg.monitoring.telegram_bot_token.clone(),
        telegram_chat_id: cfg.monitoring.telegram_chat_id.clone(),
        risk: Arc::clone(&risk),
        book: Arc::clone(&book),
        exchange: exchange.clone(),
        control_file: Some(PathBuf::from("/tmp/aria.control")),
    });

    let _watchdog = crypto_scalper::agents::watchdog::spawn(bus.clone(), WatchdogConfig::default());

    let _ = telegram
        .send(&format!(
            "🤖 *ARIA started* — multi-agent mode `{}` (manager: `{}`), pairs: {}",
            cfg.mode.run_mode,
            if cfg.manager.enabled { "ON" } else { "OFF" },
            cfg.pairs.symbols.join(", ")
        ))
        .await;

    info!(
        symbols = ?cfg.pairs.symbols,
        manager = cfg.manager.enabled,
        "all agents spawned — runtime live"
    );

    // --- Midnight daily-reset task ---
    // Without this, RiskManager.realized_pnl_today accumulates forever
    // and the daily loss circuit trips permanently after a bad day.
    {
        let bus_reset = bus.clone();
        let risk_reset = Arc::clone(&risk);
        tokio::spawn(async move {
            loop {
                let now = chrono::Utc::now();
                let tomorrow = (now.date_naive() + chrono::Days::new(1))
                    .and_hms_opt(0, 0, 30)
                    .expect("valid midnight")
                    .and_utc();
                let secs = (tomorrow - now).num_seconds().max(1) as u64;
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                risk_reset.reset_daily();
                bus_reset.publish(AgentEvent::ControlCommand(ControlCommand::ResetDaily));
                tracing::info!("midnight UTC: daily risk counters reset");
            }
        });
    }

    // --- Wait for shutdown ---
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for ctrl-c")?;
    info!("ctrl-c received — broadcasting shutdown to all agents");
    bus.publish(AgentEvent::Shutdown);
    // Give agents a moment to drain.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(())
}
