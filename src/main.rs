//! ARIA — Autonomous Realtime Intelligence Analyst
//!
//! Top-level binary. Loads config and starts the multi-agent runtime:
//! every layer of the stack runs as an independent tokio task that
//! communicates exclusively over a typed `MessageBus`. The
//! `TraderManagerAgent` (when enabled) sits between the brain and the
//! exchange and gives the final approve / veto / adjust verdict.

use anyhow::{Context, Result};
use crypto_scalper::{
    agents::{bus::MessageBus, manager::ManagerAgentConfig, messages::AgentEvent},
    backtest::{load_csv, BacktestEngine},
    config::Config,
    data::Timeframe,
    execution::risk::RiskLimits,
    execution::{binance::BinanceFutures, Exchange, PaperExchange, PositionBook, RiskManager},
    feeds::{
        ExternalSnapshot, FearGreedClient, FundingClient, NewsClient, OnchainClient,
        SentimentClient,
    },
    learning::{lessons::LessonConfig, LearningPolicy},
    llm::engine::{LlmEngine, LlmEngineConfig, LlmProvider},
    monitoring::{
        logger::TradeJournal, spawn_dashboard_server, DashboardState, MetricsState,
        TelegramNotifier,
    },
    strategy::state::{StrategyName, SymbolState},
};
use parking_lot::RwLock as PlRwLock;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
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
        };
        let result = engine.run(&candles)?;
        info!(
            symbol = %symbol,
            trades = result.trades.len(),
            win_rate = %format!("{:.2}%", result.metrics.win_rate * 100.0),
            pf = %format!("{:.2}", result.metrics.profit_factor),
            net = %format!("{:.2}", result.metrics.net_pnl),
            "backtest symbol done"
        );
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
        Arc::new(PaperExchange::new(2.0))
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

    let active: Vec<StrategyName> = cfg
        .strategy
        .active
        .iter()
        .filter_map(|s| StrategyName::parse(s))
        .collect();

    let policy = LearningPolicy::default();
    let feeds_cache: Arc<PlRwLock<HashMap<String, ExternalSnapshot>>> =
        Arc::new(PlRwLock::new(HashMap::new()));

    // --- Dashboard server ---
    let _metrics_handle = spawn_dashboard_server(
        DashboardState {
            metrics: Arc::clone(&metrics),
            policy: Some(policy.clone()),
        },
        bind,
    );

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
        },
        cfg.pairs.symbols.clone(),
        60,
    );
    let _signal =
        crypto_scalper::agents::signal::spawn(bus.clone(), Arc::clone(&states), active.clone());
    let _risk = crypto_scalper::agents::risk::spawn(
        bus.clone(),
        Arc::clone(&risk),
        policy.clone(),
        cfg.strategy.min_ta_confidence,
        cfg.llm.min_confidence,
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
        },
        policy.clone(),
        Arc::clone(&feeds_cache),
    );
    let _execution = crypto_scalper::agents::execution::spawn(
        bus.clone(),
        exchange,
        Arc::clone(&risk),
        Arc::clone(&book),
    );
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
    );

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
