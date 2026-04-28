//! ARIA — Autonomous Realtime Intelligence Analyst
//!
//! Top-level binary. Loads config, starts the data stream, and drives the
//! decision loop: pre-signal → LLM → risk gate → order dispatch → journal.

use anyhow::{Context, Result};
use chrono::Utc;
use crypto_scalper::{
    backtest::{load_csv, BacktestEngine},
    config::Config,
    data::{ws_client::WsClient, OhlcvBuilder, Side, Timeframe, WsEvent},
    execution::risk::RiskLimits,
    execution::{
        binance::BinanceFutures, orders::OrderType, Exchange, OrderRequest, PaperExchange,
        Position, PositionBook, PositionExitReason, RiskManager,
    },
    feeds::{
        ExternalSnapshot, FearGreedClient, FundingClient, NewsClient, OnchainClient,
        SentimentClient,
    },
    llm::{
        engine::{LlmEngine, LlmEngineConfig},
        ContextBuilder, Decision,
    },
    monitoring::{
        logger::TradeJournal, spawn_metrics_server, MetricsState, TelegramNotifier, TradeRecord,
    },
    strategy::{
        ema_ribbon::EmaRibbon,
        mean_reversion::MeanReversion,
        momentum::Momentum,
        select_strategies,
        squeeze::Squeeze,
        state::{PreSignal, StrategyName, SymbolState},
        vwap_scalp::VwapScalp,
        RegimeDetector, Strategy,
    },
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, Mutex};
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
        _ => run_live(cfg).await,
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

struct Runtime {
    cfg: Config,
    exchange: Arc<dyn Exchange>,
    risk: RiskManager,
    book: Arc<PositionBook>,
    journal: Arc<TradeJournal>,
    telegram: Arc<TelegramNotifier>,
    metrics: Arc<MetricsState>,
    llm: Arc<LlmEngine>,
    states: Mutex<HashMap<String, SymbolState>>,
    builders: Mutex<HashMap<String, OhlcvBuilder>>,
    fear_greed: FearGreedClient,
    funding: FundingClient,
    news: NewsClient,
    sentiment: SentimentClient,
    onchain: OnchainClient,
    active_strategies: Vec<StrategyName>,
}

async fn run_live(cfg: Config) -> Result<()> {
    // --- Init exchange ---
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

    // --- Risk manager ---
    let risk = RiskManager::new(
        RiskLimits {
            risk_per_trade_pct: cfg.risk.risk_per_trade_pct,
            max_open_positions: cfg.risk.max_open_positions,
            max_daily_loss_pct: cfg.risk.max_daily_loss_pct,
            max_drawdown_pct: cfg.risk.max_drawdown_pct,
            max_leverage: cfg.risk.max_leverage,
            max_spread_pct: cfg.risk.max_spread_pct,
        },
        cfg.risk.equity_usd,
    );

    // --- Journal ---
    let journal = Arc::new(TradeJournal::open(&cfg.monitoring.db_path)?);

    // --- Telegram ---
    let telegram = Arc::new(TelegramNotifier::new(
        cfg.monitoring.telegram_bot_token.clone(),
        std::env::var("TELEGRAM_CHAT_ID")
            .unwrap_or_else(|_| cfg.monitoring.telegram_chat_id.clone()),
    ));

    // --- Metrics server ---
    let metrics = MetricsState::new(&cfg.mode.run_mode);
    let bind = cfg
        .monitoring
        .metrics_bind
        .parse::<std::net::SocketAddr>()
        .context("invalid metrics bind")?;
    let _metrics_handle = spawn_metrics_server(Arc::clone(&metrics), bind);

    // --- LLM engine ---
    let llm = Arc::new(LlmEngine::new(LlmEngineConfig {
        api_key: cfg.llm.api_key.clone(),
        api_base: cfg.llm.api_base.clone(),
        model: cfg.llm.model.clone(),
        timeout_secs: cfg.llm.timeout_secs,
        max_tokens: cfg.llm.max_tokens,
        fallback_ta_threshold: cfg.llm.fallback_ta_threshold,
    }));

    // --- Feeds ---
    let fear_greed = FearGreedClient::new();
    let funding = FundingClient::new(cfg.exchange.rest_base_url.clone());
    let news = NewsClient::new(
        Some(cfg.feeds.cryptopanic_api_key.clone()).filter(|s| !s.is_empty()),
        cfg.feeds.rss_feeds.clone(),
    );
    let sentiment =
        SentimentClient::new(Some(cfg.feeds.lunarcrush_api_key.clone()).filter(|s| !s.is_empty()));
    let onchain = OnchainClient::new(
        Some(cfg.feeds.glassnode_api_key.clone()).filter(|s| !s.is_empty()),
        Some(cfg.feeds.whalealert_api_key.clone()).filter(|s| !s.is_empty()),
    );

    // --- State per symbol ---
    let mut states = HashMap::new();
    let mut builders = HashMap::new();
    let interval_secs = cfg
        .pairs
        .timeframes
        .first()
        .and_then(|s| Timeframe::parse(s).ok())
        .map(|t| t.seconds)
        .unwrap_or(300);
    for s in &cfg.pairs.symbols {
        states.insert(s.clone(), SymbolState::new(s));
        builders.insert(s.clone(), OhlcvBuilder::new(interval_secs));
    }

    let active: Vec<StrategyName> = cfg
        .strategy
        .active
        .iter()
        .filter_map(|s| StrategyName::parse(s))
        .collect();

    let rt = Arc::new(Runtime {
        cfg: cfg.clone(),
        exchange,
        risk,
        book: Arc::new(PositionBook::new()),
        journal,
        telegram,
        metrics,
        llm,
        states: Mutex::new(states),
        builders: Mutex::new(builders),
        fear_greed,
        funding,
        news,
        sentiment,
        onchain,
        active_strategies: active,
    });
    let _ = interval_secs; // silence unused in some branches

    // --- WS stream ---
    let (tx, mut rx) = mpsc::channel::<WsEvent>(4096);
    let ws = WsClient::new(cfg.exchange.ws_base_url.clone(), cfg.pairs.symbols.clone());
    let _ws_handle = tokio::spawn(async move { ws.run(tx).await });

    rt.telegram
        .send(&format!(
            "🤖 *ARIA started* — mode `{}`, pairs: {}",
            cfg.mode.run_mode,
            cfg.pairs.symbols.join(", ")
        ))
        .await
        .ok();

    while let Some(event) = rx.recv().await {
        match event {
            WsEvent::Trade { symbol, trade } => {
                let finalized_candle = {
                    let mut builders = rt.builders.lock().await;
                    builders.get_mut(&symbol).and_then(|b| b.ingest(trade))
                };

                // Check exits on every trade (using mark price)
                for (pos, reason) in rt.book.check_exits(&symbol, trade.price) {
                    close_position(&rt, pos, trade.price, reason).await;
                }

                if let Some(c) = finalized_candle {
                    let (signal, regime, ta_cfg) = {
                        let mut states = rt.states.lock().await;
                        let state = states.get_mut(&symbol).unwrap();
                        state.on_closed(c);
                        let regime = RegimeDetector::detect(state);
                        let chosen = select_strategies(&rt.active_strategies, regime);
                        let mut best: Option<PreSignal> = None;
                        for name in chosen {
                            let sig = match name {
                                StrategyName::EmaRibbon => EmaRibbon.evaluate(state, &c),
                                StrategyName::MeanReversion => MeanReversion.evaluate(state, &c),
                                StrategyName::Momentum => Momentum.evaluate(state, &c),
                                StrategyName::VwapScalp => VwapScalp.evaluate(state, &c),
                                StrategyName::Squeeze => Squeeze.evaluate(state, &c),
                            };
                            if let Some(s) = sig {
                                if best
                                    .as_ref()
                                    .map(|b| s.ta_confidence > b.ta_confidence)
                                    .unwrap_or(true)
                                {
                                    best = Some(s);
                                }
                            }
                        }
                        (best, regime, rt.cfg.strategy.min_ta_confidence)
                    };

                    if let Some(signal) = signal {
                        if signal.ta_confidence < ta_cfg {
                            continue;
                        }
                        if let Err(e) = rt.risk.can_open_position() {
                            warn!(symbol = %symbol, reason = %e, "risk gate blocked");
                            continue;
                        }
                        rt.metrics.update(|m| m.signals_today += 1);
                        let rt2 = Arc::clone(&rt);
                        let sig_clone = signal.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_signal(&rt2, sig_clone, regime).await {
                                warn!(error = %e, "handle_signal");
                            }
                        });
                    }
                }
            }
            WsEvent::BookTicker {
                symbol,
                best_bid,
                best_ask,
            } => {
                let mut states = rt.states.lock().await;
                if let Some(state) = states.get_mut(&symbol) {
                    state.order_book.set_top(best_bid, best_ask);
                }
            }
            WsEvent::Heartbeat => {}
            WsEvent::Disconnected(reason) => {
                warn!(%reason, "ws disconnected — reconnect pending");
            }
        }
    }

    Ok(())
}

async fn handle_signal(
    rt: &Runtime,
    signal: PreSignal,
    regime: crypto_scalper::strategy::Regime,
) -> Result<()> {
    let symbol = signal.symbol.clone();

    // 1. Gather external data in parallel
    let base_owned: String = symbol.trim_end_matches("USDT").to_string();
    let base_slice: [&str; 1] = [base_owned.as_str()];
    let (fg, news, sent, onc, fund) = tokio::join!(
        rt.fear_greed.fetch(),
        rt.news.fetch(&base_slice),
        rt.sentiment.fetch(&symbol),
        rt.onchain.fetch(&symbol),
        rt.funding.fetch(&symbol),
    );
    let external = ExternalSnapshot {
        fear_greed: fg.ok(),
        news: news.ok(),
        sentiment: sent.ok(),
        onchain: onc.ok(),
        funding: fund.ok(),
    };

    // 2. Build context and call LLM
    let ctx = {
        let states = rt.states.lock().await;
        let state = states.get(&symbol).unwrap();
        ContextBuilder::build(state, regime, &signal, external.clone())
    };
    let llm_out = rt.llm.analyze(&ctx).await?;

    rt.metrics.update(|m| {
        let n = m.llm_go + m.llm_nogo + m.llm_wait;
        let avg = m.llm_avg_confidence * n as f64 + llm_out.decision.confidence as f64;
        match llm_out.decision.decision {
            Decision::Go => m.llm_go += 1,
            Decision::NoGo => m.llm_nogo += 1,
            Decision::Wait => m.llm_wait += 1,
        }
        m.llm_avg_confidence = avg / ((n + 1) as f64).max(1.0);
        let total = m.llm_go + m.llm_nogo + m.llm_wait;
        m.llm_avg_latency_ms =
            (m.llm_avg_latency_ms * (total.saturating_sub(1)) + llm_out.latency_ms) / total.max(1);
        if llm_out.offline_fallback {
            m.llm_offline_fallbacks += 1;
        }
    });

    rt.journal.log_llm_decision(
        &symbol,
        signal.strategy.as_str(),
        regime.as_str(),
        &llm_out.decision.direction,
        signal.ta_confidence,
        format!("{:?}", llm_out.decision.decision).as_str(),
        llm_out.decision.confidence,
        llm_out.decision.market_context_score.composite_score,
        &llm_out.decision.reasoning.summary,
        &serde_json::to_string(&llm_out.decision).unwrap_or_default(),
        llm_out.latency_ms,
        llm_out.offline_fallback,
    )?;

    // 3. Apply LLM gate
    if llm_out.decision.decision != Decision::Go {
        info!(
            symbol = %symbol,
            decision = ?llm_out.decision.decision,
            conf = llm_out.decision.confidence,
            "signal rejected by LLM"
        );
        return Ok(());
    }
    if llm_out.decision.confidence < rt.cfg.llm.min_confidence {
        info!(
            symbol = %symbol,
            conf = llm_out.decision.confidence,
            threshold = rt.cfg.llm.min_confidence,
            "LLM confidence below threshold"
        );
        return Ok(());
    }

    // 4. Size and dispatch order
    let entry = llm_out.decision.entry_price.unwrap_or(signal.entry);
    let size = rt.risk.calculate_size(entry, signal.stop_loss);
    if size <= 0.0 {
        warn!(symbol = %symbol, "risk size == 0 — skipping");
        return Ok(());
    }

    let client_id = format!("aria-{}-{}", symbol, Utc::now().timestamp_millis());
    let req = OrderRequest {
        client_id: client_id.clone(),
        symbol: symbol.clone(),
        side: signal.side,
        size,
        price: Some(entry),
        stop_loss: signal.stop_loss,
        take_profit: signal.take_profit,
        order_type: OrderType::Market,
    };

    let ack = match rt.exchange.place_order(&req).await {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "place_order failed");
            return Ok(());
        }
    };

    rt.risk.on_position_opened();
    let pos = Position {
        client_id: client_id.clone(),
        symbol: symbol.clone(),
        side: signal.side,
        size,
        entry_price: ack.avg_fill_price.max(entry),
        stop_loss: signal.stop_loss,
        take_profit: signal.take_profit,
        opened_at: Utc::now(),
        trailing_activated: false,
        peak_price: entry,
        trough_price: entry,
    };
    rt.book.open(pos);

    // 5. Journal opening
    let record = build_open_record(&req, &ack, &signal, regime, &ctx, &llm_out);
    rt.journal.insert_trade(&record)?;
    rt.telegram
        .send(&format!(
            "🟢 *OPEN* `{}` {} size `{:.4}` entry `{:.2}` SL `{:.2}` TP `{:.2}` conf {}",
            symbol,
            signal.side.as_str(),
            size,
            ack.avg_fill_price.max(entry),
            signal.stop_loss,
            signal.take_profit,
            llm_out.decision.confidence
        ))
        .await
        .ok();

    Ok(())
}

fn build_open_record(
    req: &OrderRequest,
    ack: &crypto_scalper::execution::exchange::OrderAck,
    signal: &PreSignal,
    regime: crypto_scalper::strategy::Regime,
    ctx: &crypto_scalper::llm::MarketContext,
    llm_out: &crypto_scalper::llm::engine::LlmCallResult,
) -> TradeRecord {
    TradeRecord {
        client_order_id: req.client_id.clone(),
        symbol: req.symbol.clone(),
        direction: signal.side.as_str().to_string(),
        strategy: signal.strategy.as_str().to_string(),
        market_regime: regime.as_str().to_string(),
        entry_time: Utc::now(),
        entry_price: ack.avg_fill_price,
        size: req.size,
        stop_loss: req.stop_loss,
        take_profit: req.take_profit,
        exit_time: None,
        exit_price: None,
        exit_reason: None,
        pnl_usd: None,
        pnl_pct: None,
        fees_paid: Some(ack.fee_usd),
        ta_confidence: Some(signal.ta_confidence),
        rsi: ctx.rsi,
        adx: ctx.adx,
        vwap_delta_pct: ctx.vwap.map(|v| (ctx.current_price - v) / v * 100.0),
        ema_alignment: Some(regime.as_str().to_string()),
        llm_model: Some(llm_out.decision.direction.clone()),
        llm_decision: Some(format!("{:?}", llm_out.decision.decision)),
        llm_confidence: Some(llm_out.decision.confidence),
        llm_ta_score: Some(llm_out.decision.market_context_score.ta_score),
        llm_sentiment_score: Some(llm_out.decision.market_context_score.sentiment_score),
        llm_fundamental_score: Some(llm_out.decision.market_context_score.fundamental_score),
        llm_composite: Some(llm_out.decision.market_context_score.composite_score),
        llm_summary: Some(llm_out.decision.reasoning.summary.clone()),
        llm_ta_analysis: Some(llm_out.decision.reasoning.ta_analysis.clone()),
        llm_sentiment: Some(llm_out.decision.reasoning.sentiment_analysis.clone()),
        llm_fundamental: Some(llm_out.decision.reasoning.fundamental_analysis.clone()),
        llm_risks: Some(llm_out.decision.reasoning.risk_factors.clone()),
        llm_invalidation: Some(llm_out.decision.reasoning.invalidation.clone()),
        llm_latency_ms: Some(llm_out.latency_ms),
        fear_greed: ctx.external.fear_greed.as_ref().map(|f| f.value),
        social_sentiment: ctx.external.sentiment.as_ref().map(|s| s.sentiment),
        news_score: ctx.external.news.as_ref().map(|n| n.net_score),
        funding_rate: ctx.external.funding.as_ref().map(|f| f.rate),
        top_news_titles: ctx.external.news.as_ref().map(|n| {
            let titles: Vec<&str> = n.items.iter().take(5).map(|i| i.title.as_str()).collect();
            serde_json::to_string(&titles).unwrap_or_default()
        }),
    }
}

async fn close_position(rt: &Runtime, pos: Position, exit_price: f64, reason: PositionExitReason) {
    let pnl = match pos.side {
        Side::Long => (exit_price - pos.entry_price) * pos.size,
        Side::Short => (pos.entry_price - exit_price) * pos.size,
    };
    let pnl_pct = match pos.side {
        Side::Long => (exit_price / pos.entry_price - 1.0) * 100.0,
        Side::Short => (pos.entry_price / exit_price - 1.0) * 100.0,
    };
    rt.risk.on_position_closed(pnl);
    let _ = rt.journal.close_trade(
        &pos.client_id,
        Utc::now(),
        exit_price,
        reason.as_str(),
        pnl,
        pnl_pct,
        0.0,
    );
    rt.metrics.update(|m| {
        let snap = rt.risk.snapshot();
        m.equity = snap.equity;
        m.peak_equity = snap.peak_equity;
        m.open_positions = snap.open_positions;
        m.daily_pnl = snap.realized_pnl_today;
        m.trades_today += 1;
    });
    rt.telegram
        .send(&format!(
            "🔴 *CLOSE {}* `{}` {} pnl `{:+.2}` ({:+.2}%)",
            reason.as_str(),
            pos.symbol,
            pos.side.as_str(),
            pnl,
            pnl_pct
        ))
        .await
        .ok();
}
