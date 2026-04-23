//! Prometheus-compatible metrics + HTTP endpoint + interactive web dashboard.
//!
//! Endpoints:
//!   /           — interactive web dashboard (charts, grid viz, fill history)
//!   /health     — liveness check
//!   /status     — JSON status snapshot
//!   /metrics    — Prometheus format
//!   /api/live   — full live data for dashboard (JSON)

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, Json};
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{error, info};

/// Shared metrics state.
pub struct Metrics {
    pub fills_total: AtomicU64,
    pub buy_fills: AtomicU64,
    pub sell_fills: AtomicU64,
    pub orders_placed: AtomicU64,
    pub orders_failed: AtomicU64,
    pub realised_pnl_cents: AtomicI64,
    pub equity_cents: AtomicI64,
    pub open_orders: AtomicU64,
    pub circuit_breaker_trips: AtomicU64,
    pub api_retries: AtomicU64,
    pub ws_reconnects: AtomicU64,
    pub uptime_start: AtomicI64,
    /// Time-series data for charts (protected by mutex).
    pub series: Mutex<DashboardSeries>,
}

/// Time-series data for the web dashboard.
pub struct DashboardSeries {
    /// (timestamp_ms, price)
    pub price_history: Vec<(i64, f64)>,
    /// (timestamp_ms, equity)
    pub equity_history: Vec<(i64, f64)>,
    /// (timestamp_ms, cumulative_pnl)
    pub pnl_history: Vec<(i64, f64)>,
    /// Recent fill events for the table
    pub recent_fills: Vec<FillEntry>,
    /// Current grid levels with status
    pub grid_levels: Vec<GridLevel>,
    /// Current symbol
    pub symbol: String,
    /// Current regime
    pub regime: String,
    /// Regime confidence
    pub regime_confidence: f64,
    /// Whether grid is suitable
    pub grid_suitable: bool,
}

#[derive(Clone, Serialize)]
pub struct FillEntry {
    pub timestamp: i64,
    pub side: String,
    pub price: f64,
    pub qty: f64,
    pub pnl: f64,
    pub order_id: String,
}

#[derive(Clone, Serialize)]
pub struct GridLevel {
    pub index: usize,
    pub price: f64,
    pub side: String,   // "BUY" / "SELL" / ""
    pub status: String, // "OPEN" / ""
}

impl DashboardSeries {
    pub fn new() -> Self {
        Self {
            price_history: Vec::new(),
            equity_history: Vec::new(),
            pnl_history: Vec::new(),
            recent_fills: Vec::new(),
            grid_levels: Vec::new(),
            symbol: String::new(),
            regime: "UNKNOWN".to_string(),
            regime_confidence: 0.0,
            grid_suitable: true,
        }
    }

    pub fn push_price(&mut self, price: f64) {
        let ts = chrono::Utc::now().timestamp_millis();
        self.price_history.push((ts, price));
        // Keep last 1000 points
        if self.price_history.len() > 1000 {
            self.price_history.drain(..self.price_history.len() - 1000);
        }
    }

    pub fn push_equity(&mut self, equity: f64) {
        let ts = chrono::Utc::now().timestamp_millis();
        self.equity_history.push((ts, equity));
        if self.equity_history.len() > 1000 {
            self.equity_history.drain(..self.equity_history.len() - 1000);
        }
    }

    pub fn push_pnl(&mut self, pnl: f64) {
        let ts = chrono::Utc::now().timestamp_millis();
        self.pnl_history.push((ts, pnl));
        if self.pnl_history.len() > 1000 {
            self.pnl_history.drain(..self.pnl_history.len() - 1000);
        }
    }

    pub fn add_fill(&mut self, fill: FillEntry) {
        self.recent_fills.push(fill);
        // Keep last 100 fills
        if self.recent_fills.len() > 100 {
            self.recent_fills.drain(..self.recent_fills.len() - 100);
        }
    }
}

impl Metrics {
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            fills_total: AtomicU64::new(0),
            buy_fills: AtomicU64::new(0),
            sell_fills: AtomicU64::new(0),
            orders_placed: AtomicU64::new(0),
            orders_failed: AtomicU64::new(0),
            realised_pnl_cents: AtomicI64::new(0),
            equity_cents: AtomicI64::new(0),
            open_orders: AtomicU64::new(0),
            circuit_breaker_trips: AtomicU64::new(0),
            api_retries: AtomicU64::new(0),
            ws_reconnects: AtomicU64::new(0),
            uptime_start: AtomicI64::new(now),
            series: Mutex::new(DashboardSeries::new()),
        }
    }

    pub fn record_fill(&self, is_buy: bool) {
        self.fills_total.fetch_add(1, Ordering::Relaxed);
        if is_buy {
            self.buy_fills.fetch_add(1, Ordering::Relaxed);
        } else {
            self.sell_fills.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_order_placed(&self) {
        self.orders_placed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_order_failed(&self) {
        self.orders_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_pnl(&self, pnl: f64) {
        self.realised_pnl_cents
            .store((pnl * 100.0) as i64, Ordering::Relaxed);
    }

    pub fn set_equity(&self, equity: f64) {
        self.equity_cents
            .store((equity * 100.0) as i64, Ordering::Relaxed);
    }

    pub fn set_open_orders(&self, count: u64) {
        self.open_orders.store(count, Ordering::Relaxed);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

// -- JSON API types ----------------------------------------------------------

#[derive(Serialize)]
struct StatusResponse {
    uptime_secs: i64,
    fills_total: u64,
    buy_fills: u64,
    sell_fills: u64,
    orders_placed: u64,
    orders_failed: u64,
    realised_pnl: f64,
    equity: f64,
    open_orders: u64,
    circuit_breaker_trips: u64,
    api_retries: u64,
    ws_reconnects: u64,
}

#[derive(Serialize)]
struct LiveData {
    // Snapshot
    symbol: String,
    uptime_secs: i64,
    equity: f64,
    realised_pnl: f64,
    fills_total: u64,
    buy_fills: u64,
    sell_fills: u64,
    open_orders: u64,
    orders_placed: u64,
    orders_failed: u64,
    regime: String,
    regime_confidence: f64,
    grid_suitable: bool,
    // Time series
    price_history: Vec<(i64, f64)>,
    equity_history: Vec<(i64, f64)>,
    pnl_history: Vec<(i64, f64)>,
    // Grid
    grid_levels: Vec<GridLevel>,
    // Fills
    recent_fills: Vec<FillEntry>,
}

// -- Handlers ----------------------------------------------------------------

async fn health_handler() -> &'static str {
    "OK"
}

async fn status_handler(State(m): State<Arc<Metrics>>) -> Json<StatusResponse> {
    let now = chrono::Utc::now().timestamp();
    let start = m.uptime_start.load(Ordering::Relaxed);
    Json(StatusResponse {
        uptime_secs: now - start,
        fills_total: m.fills_total.load(Ordering::Relaxed),
        buy_fills: m.buy_fills.load(Ordering::Relaxed),
        sell_fills: m.sell_fills.load(Ordering::Relaxed),
        orders_placed: m.orders_placed.load(Ordering::Relaxed),
        orders_failed: m.orders_failed.load(Ordering::Relaxed),
        realised_pnl: m.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0,
        equity: m.equity_cents.load(Ordering::Relaxed) as f64 / 100.0,
        open_orders: m.open_orders.load(Ordering::Relaxed),
        circuit_breaker_trips: m.circuit_breaker_trips.load(Ordering::Relaxed),
        api_retries: m.api_retries.load(Ordering::Relaxed),
        ws_reconnects: m.ws_reconnects.load(Ordering::Relaxed),
    })
}

async fn live_handler(State(m): State<Arc<Metrics>>) -> Json<LiveData> {
    let now = chrono::Utc::now().timestamp();
    let start = m.uptime_start.load(Ordering::Relaxed);
    let series = m.series.lock().await;

    Json(LiveData {
        symbol: series.symbol.clone(),
        uptime_secs: now - start,
        equity: m.equity_cents.load(Ordering::Relaxed) as f64 / 100.0,
        realised_pnl: m.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0,
        fills_total: m.fills_total.load(Ordering::Relaxed),
        buy_fills: m.buy_fills.load(Ordering::Relaxed),
        sell_fills: m.sell_fills.load(Ordering::Relaxed),
        open_orders: m.open_orders.load(Ordering::Relaxed),
        orders_placed: m.orders_placed.load(Ordering::Relaxed),
        orders_failed: m.orders_failed.load(Ordering::Relaxed),
        regime: series.regime.clone(),
        regime_confidence: series.regime_confidence,
        grid_suitable: series.grid_suitable,
        price_history: series.price_history.clone(),
        equity_history: series.equity_history.clone(),
        pnl_history: series.pnl_history.clone(),
        grid_levels: series.grid_levels.clone(),
        recent_fills: series.recent_fills.clone(),
    })
}

async fn metrics_handler(State(m): State<Arc<Metrics>>) -> String {
    let now = chrono::Utc::now().timestamp();
    let start = m.uptime_start.load(Ordering::Relaxed);
    format!(
        "# HELP trading_bot_fills_total Total number of fills\n\
         # TYPE trading_bot_fills_total counter\n\
         trading_bot_fills_total {}\n\
         # HELP trading_bot_buy_fills Total buy fills\n\
         # TYPE trading_bot_buy_fills counter\n\
         trading_bot_buy_fills {}\n\
         # HELP trading_bot_sell_fills Total sell fills\n\
         # TYPE trading_bot_sell_fills counter\n\
         trading_bot_sell_fills {}\n\
         # HELP trading_bot_orders_placed Total orders placed\n\
         # TYPE trading_bot_orders_placed counter\n\
         trading_bot_orders_placed {}\n\
         # HELP trading_bot_orders_failed Total orders failed\n\
         # TYPE trading_bot_orders_failed counter\n\
         trading_bot_orders_failed {}\n\
         # HELP trading_bot_realised_pnl Realised P&L in quote currency\n\
         # TYPE trading_bot_realised_pnl gauge\n\
         trading_bot_realised_pnl {:.2}\n\
         # HELP trading_bot_equity Current equity in quote currency\n\
         # TYPE trading_bot_equity gauge\n\
         trading_bot_equity {:.2}\n\
         # HELP trading_bot_open_orders Current open orders\n\
         # TYPE trading_bot_open_orders gauge\n\
         trading_bot_open_orders {}\n\
         # HELP trading_bot_uptime_seconds Bot uptime in seconds\n\
         # TYPE trading_bot_uptime_seconds gauge\n\
         trading_bot_uptime_seconds {}\n",
        m.fills_total.load(Ordering::Relaxed),
        m.buy_fills.load(Ordering::Relaxed),
        m.sell_fills.load(Ordering::Relaxed),
        m.orders_placed.load(Ordering::Relaxed),
        m.orders_failed.load(Ordering::Relaxed),
        m.realised_pnl_cents.load(Ordering::Relaxed) as f64 / 100.0,
        m.equity_cents.load(Ordering::Relaxed) as f64 / 100.0,
        m.open_orders.load(Ordering::Relaxed),
        now - start,
    )
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

/// Start the HTTP metrics + dashboard server on the given port.
pub async fn serve(metrics: Arc<Metrics>, port: u16) {
    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/health", get(health_handler))
        .route("/status", get(status_handler))
        .route("/metrics", get(metrics_handler))
        .route("/api/live", get(live_handler))
        .with_state(metrics);

    let addr = format!("0.0.0.0:{port}");
    info!(addr = %addr, "dashboard + metrics HTTP server starting");
    match TcpListener::bind(&addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(listener, app).await {
                error!(err = %e, "metrics server error");
            }
        }
        Err(e) => {
            error!(err = %e, addr = %addr, "failed to bind metrics server");
        }
    }
}

// -- Embedded dashboard HTML -------------------------------------------------

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Trading Bot Dashboard</title>
<style>
  :root {
    --bg: #0d1117; --card: #161b22; --border: #30363d;
    --text: #e6edf3; --muted: #8b949e; --green: #3fb950;
    --red: #f85149; --blue: #58a6ff; --yellow: #d29922;
    --cyan: #39d2c0; --purple: #bc8cff;
  }
  * { margin:0; padding:0; box-sizing:border-box; }
  body { font-family: -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;
    background: var(--bg); color: var(--text); min-height: 100vh; }
  .header { background: var(--card); border-bottom: 1px solid var(--border);
    padding: 12px 24px; display: flex; align-items: center; justify-content: space-between; }
  .header h1 { font-size: 18px; font-weight: 600; }
  .header h1 span { color: var(--cyan); }
  .header .status { display: flex; gap: 16px; align-items: center; font-size: 13px; }
  .header .dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; margin-right: 4px; }
  .dot.live { background: var(--green); animation: pulse 2s infinite; }
  .dot.dead { background: var(--red); }
  @keyframes pulse { 0%,100%{opacity:1} 50%{opacity:0.4} }

  .grid-layout { display: grid; grid-template-columns: 1fr 1fr 1fr 1fr; gap: 12px;
    padding: 16px 24px; }
  .stat-card { background: var(--card); border: 1px solid var(--border); border-radius: 8px;
    padding: 16px; }
  .stat-card .label { font-size: 11px; color: var(--muted); text-transform: uppercase;
    letter-spacing: 0.5px; margin-bottom: 4px; }
  .stat-card .value { font-size: 24px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .stat-card .value.green { color: var(--green); }
  .stat-card .value.red { color: var(--red); }
  .stat-card .sub { font-size: 11px; color: var(--muted); margin-top: 2px; }

  .charts { display: grid; grid-template-columns: 2fr 1fr; gap: 12px;
    padding: 0 24px 12px; }
  .chart-card { background: var(--card); border: 1px solid var(--border); border-radius: 8px;
    padding: 16px; }
  .chart-card h3 { font-size: 13px; color: var(--muted); margin-bottom: 8px; }
  canvas { width: 100% !important; height: 200px !important; }

  .bottom { display: grid; grid-template-columns: 1fr 1fr; gap: 12px;
    padding: 0 24px 24px; }
  .table-card { background: var(--card); border: 1px solid var(--border); border-radius: 8px;
    padding: 16px; overflow: hidden; }
  .table-card h3 { font-size: 13px; color: var(--muted); margin-bottom: 8px; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 6px 8px;
    border-bottom: 1px solid var(--border); }
  td { padding: 5px 8px; border-bottom: 1px solid var(--border); font-variant-numeric: tabular-nums; }
  tr.buy td { color: var(--green); }
  tr.sell td { color: var(--red); }
  tr.current { background: rgba(88,166,255,0.08); }
  .badge { display: inline-block; padding: 2px 8px; border-radius: 12px; font-size: 11px;
    font-weight: 600; }
  .badge.ranging { background: rgba(63,185,80,0.15); color: var(--green); }
  .badge.trending-up { background: rgba(210,153,34,0.15); color: var(--yellow); }
  .badge.trending-down { background: rgba(248,81,73,0.15); color: var(--red); }
  .badge.volatile { background: rgba(248,81,73,0.25); color: var(--red); }
  .badge.unknown { background: rgba(139,148,158,0.15); color: var(--muted); }

  .grid-viz { display: flex; flex-direction: column; gap: 2px; max-height: 400px;
    overflow-y: auto; }
  .grid-level { display: flex; align-items: center; padding: 3px 8px; border-radius: 4px;
    font-size: 11px; font-variant-numeric: tabular-nums; }
  .grid-level.buy { background: rgba(63,185,80,0.08); }
  .grid-level.sell { background: rgba(248,81,73,0.08); }
  .grid-level.current { border: 1px solid var(--blue); background: rgba(88,166,255,0.1); }
  .grid-level .idx { width: 30px; color: var(--muted); }
  .grid-level .price { flex: 1; }
  .grid-level .side-tag { width: 40px; text-align: center; font-weight: 600; }
  .grid-level .side-tag.buy { color: var(--green); }
  .grid-level .side-tag.sell { color: var(--red); }

  @media (max-width: 900px) {
    .grid-layout { grid-template-columns: 1fr 1fr; }
    .charts, .bottom { grid-template-columns: 1fr; }
  }
</style>
</head>
<body>

<div class="header">
  <h1><span>&#9670;</span> Trading Bot</h1>
  <div class="status">
    <span id="symbol">---</span>
    <span id="regime-badge" class="badge unknown">---</span>
    <span><span id="dot" class="dot live"></span><span id="uptime">0h 0m</span></span>
  </div>
</div>

<div class="grid-layout">
  <div class="stat-card">
    <div class="label">Equity</div>
    <div class="value" id="equity">---</div>
    <div class="sub">USDT</div>
  </div>
  <div class="stat-card">
    <div class="label">Realised P&amp;L</div>
    <div class="value" id="pnl">---</div>
    <div class="sub" id="pnl-per-fill">---</div>
  </div>
  <div class="stat-card">
    <div class="label">Total Fills</div>
    <div class="value" id="fills">---</div>
    <div class="sub" id="fills-detail">--- buys / --- sells</div>
  </div>
  <div class="stat-card">
    <div class="label">Open Orders</div>
    <div class="value" id="open-orders">---</div>
    <div class="sub" id="orders-detail">--- placed / --- failed</div>
  </div>
</div>

<div class="charts">
  <div class="chart-card">
    <h3>Price &amp; Grid Levels</h3>
    <canvas id="priceChart"></canvas>
  </div>
  <div class="chart-card">
    <h3>P&amp;L Over Time</h3>
    <canvas id="pnlChart"></canvas>
  </div>
</div>

<div class="bottom">
  <div class="table-card">
    <h3>Grid Levels</h3>
    <div class="grid-viz" id="grid-viz"></div>
  </div>
  <div class="table-card">
    <h3>Recent Fills</h3>
    <table>
      <thead><tr><th>Time</th><th>Side</th><th>Price</th><th>Qty</th><th>P&amp;L</th></tr></thead>
      <tbody id="fills-table"></tbody>
    </table>
  </div>
</div>

<script>
// Minimal chart library — canvas-based, no dependencies
class MiniChart {
  constructor(canvas, opts = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d');
    this.color = opts.color || '#58a6ff';
    this.fillColor = opts.fillColor || 'rgba(88,166,255,0.1)';
    this.gridLines = opts.gridLines || [];
    this.gridLineColors = opts.gridLineColors || [];
    this.data = [];
  }

  setData(points) { this.data = points; this.draw(); }
  setGridLines(lines, colors) { this.gridLines = lines; this.gridLineColors = colors; }

  draw() {
    const c = this.canvas, ctx = this.ctx;
    const dpr = window.devicePixelRatio || 1;
    const rect = c.getBoundingClientRect();
    c.width = rect.width * dpr;
    c.height = rect.height * dpr;
    ctx.scale(dpr, dpr);
    const W = rect.width, H = rect.height;

    ctx.clearRect(0, 0, W, H);
    if (this.data.length < 2) {
      ctx.fillStyle = '#8b949e';
      ctx.font = '12px sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('Waiting for data...', W/2, H/2);
      return;
    }

    const vals = this.data.map(d => d[1]);
    let allVals = [...vals, ...this.gridLines];
    let min = Math.min(...allVals), max = Math.max(...allVals);
    if (min === max) { min -= 1; max += 1; }
    const pad = (max - min) * 0.05;
    min -= pad; max += pad;
    const range = max - min;

    const xStep = W / (this.data.length - 1);
    const toY = v => H - ((v - min) / range) * H;

    // Grid lines (buy/sell levels)
    for (let i = 0; i < this.gridLines.length; i++) {
      const y = toY(this.gridLines[i]);
      if (y >= 0 && y <= H) {
        ctx.strokeStyle = this.gridLineColors[i] || 'rgba(139,148,158,0.2)';
        ctx.lineWidth = 0.5;
        ctx.setLineDash([4, 4]);
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(W, y);
        ctx.stroke();
        ctx.setLineDash([]);
      }
    }

    // Area fill
    ctx.fillStyle = this.fillColor;
    ctx.beginPath();
    ctx.moveTo(0, H);
    for (let i = 0; i < this.data.length; i++) {
      ctx.lineTo(i * xStep, toY(this.data[i][1]));
    }
    ctx.lineTo(W, H);
    ctx.closePath();
    ctx.fill();

    // Line
    ctx.strokeStyle = this.color;
    ctx.lineWidth = 1.5;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    for (let i = 0; i < this.data.length; i++) {
      const x = i * xStep, y = toY(this.data[i][1]);
      i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
    }
    ctx.stroke();

    // Y-axis labels
    ctx.fillStyle = '#8b949e';
    ctx.font = '10px sans-serif';
    ctx.textAlign = 'left';
    const steps = 4;
    for (let i = 0; i <= steps; i++) {
      const val = min + (range * i / steps);
      const y = toY(val);
      ctx.fillText(val.toFixed(2), 4, y - 2);
    }

    // Latest value label
    const lastVal = this.data[this.data.length - 1][1];
    const lastY = toY(lastVal);
    ctx.fillStyle = this.color;
    ctx.font = 'bold 11px sans-serif';
    ctx.textAlign = 'right';
    ctx.fillText(lastVal.toFixed(2), W - 4, lastY - 4);
    ctx.beginPath();
    ctx.arc(W - 2, lastY, 3, 0, Math.PI * 2);
    ctx.fillStyle = this.color;
    ctx.fill();
  }
}

const priceChart = new MiniChart(document.getElementById('priceChart'), {
  color: '#58a6ff', fillColor: 'rgba(88,166,255,0.08)'
});
const pnlChart = new MiniChart(document.getElementById('pnlChart'), {
  color: '#3fb950', fillColor: 'rgba(63,185,80,0.08)'
});

function formatTime(ts) {
  const d = new Date(ts);
  return d.toLocaleTimeString([], {hour:'2-digit',minute:'2-digit',second:'2-digit'});
}

function formatUptime(secs) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h + 'h ' + m + 'm';
}

function regimeClass(r) {
  switch(r) {
    case 'RANGING': return 'ranging';
    case 'TRENDING_UP': return 'trending-up';
    case 'TRENDING_DOWN': return 'trending-down';
    case 'VOLATILE': return 'volatile';
    default: return 'unknown';
  }
}

function update(data) {
  // Header
  document.getElementById('symbol').textContent = data.symbol || '---';
  const rb = document.getElementById('regime-badge');
  rb.textContent = data.regime + (data.grid_suitable ? '' : ' (paused)');
  rb.className = 'badge ' + regimeClass(data.regime);
  document.getElementById('uptime').textContent = formatUptime(data.uptime_secs);

  // Stats
  document.getElementById('equity').textContent = data.equity.toFixed(2);

  const pnlEl = document.getElementById('pnl');
  pnlEl.textContent = (data.realised_pnl >= 0 ? '+' : '') + data.realised_pnl.toFixed(2);
  pnlEl.className = 'value ' + (data.realised_pnl >= 0 ? 'green' : 'red');

  const avgPnl = data.fills_total > 0 ? data.realised_pnl / data.fills_total : 0;
  document.getElementById('pnl-per-fill').textContent = 'Avg/fill: ' + avgPnl.toFixed(4) + ' USDT';

  document.getElementById('fills').textContent = data.fills_total;
  document.getElementById('fills-detail').textContent = data.buy_fills + ' buys / ' + data.sell_fills + ' sells';

  document.getElementById('open-orders').textContent = data.open_orders;
  document.getElementById('orders-detail').textContent = data.orders_placed + ' placed / ' + data.orders_failed + ' failed';

  // Price chart with grid levels as horizontal lines
  if (data.price_history.length > 0) {
    const gridLines = data.grid_levels.map(g => g.price);
    const gridColors = data.grid_levels.map(g =>
      g.side === 'BUY' ? 'rgba(63,185,80,0.3)' :
      g.side === 'SELL' ? 'rgba(248,81,73,0.3)' :
      'rgba(139,148,158,0.15)'
    );
    priceChart.setGridLines(gridLines, gridColors);
    priceChart.setData(data.price_history);
  }

  // P&L chart
  if (data.pnl_history.length > 0) {
    const lastPnl = data.pnl_history[data.pnl_history.length - 1][1];
    pnlChart.color = lastPnl >= 0 ? '#3fb950' : '#f85149';
    pnlChart.fillColor = lastPnl >= 0 ? 'rgba(63,185,80,0.08)' : 'rgba(248,81,73,0.08)';
    pnlChart.setData(data.pnl_history);
  }

  // Grid visualization
  const gridViz = document.getElementById('grid-viz');
  const currentPrice = data.price_history.length > 0
    ? data.price_history[data.price_history.length - 1][1] : 0;
  let gridHtml = '';
  // Show levels from top (highest price) to bottom
  const levels = [...data.grid_levels].reverse();
  for (const g of levels) {
    const isCurrent = currentPrice > 0 &&
      Math.abs(g.price - currentPrice) === Math.min(...data.grid_levels.map(l => Math.abs(l.price - currentPrice)));
    const cls = g.side ? g.side.toLowerCase() : '';
    const currentCls = isCurrent ? ' current' : '';
    gridHtml += '<div class="grid-level ' + cls + currentCls + '">' +
      '<span class="idx">#' + g.index + '</span>' +
      '<span class="price">' + g.price.toFixed(4) + '</span>' +
      '<span class="side-tag ' + cls + '">' + (g.side || '---') + '</span>' +
      '<span>' + (g.status || '---') + '</span>' +
      '</div>';
  }
  gridViz.innerHTML = gridHtml;

  // Fills table
  const tbody = document.getElementById('fills-table');
  let fillsHtml = '';
  const fills = [...data.recent_fills].reverse().slice(0, 20);
  for (const f of fills) {
    const cls = f.side === 'BUY' ? 'buy' : 'sell';
    fillsHtml += '<tr class="' + cls + '">' +
      '<td>' + formatTime(f.timestamp) + '</td>' +
      '<td>' + f.side + '</td>' +
      '<td>' + f.price.toFixed(4) + '</td>' +
      '<td>' + f.qty.toFixed(6) + '</td>' +
      '<td>' + (f.pnl >= 0 ? '+' : '') + f.pnl.toFixed(4) + '</td></tr>';
  }
  if (fills.length === 0) {
    fillsHtml = '<tr><td colspan="5" style="color:var(--muted);text-align:center;padding:24px">No fills yet</td></tr>';
  }
  tbody.innerHTML = fillsHtml;
}

// Poll for updates
async function poll() {
  try {
    const resp = await fetch('/api/live');
    if (resp.ok) {
      const data = await resp.json();
      update(data);
      document.getElementById('dot').className = 'dot live';
    } else {
      document.getElementById('dot').className = 'dot dead';
    }
  } catch(e) {
    document.getElementById('dot').className = 'dot dead';
  }
}

// Initial load + interval
poll();
setInterval(poll, 2000);

// Resize charts on window resize
window.addEventListener('resize', () => { priceChart.draw(); pnlChart.draw(); });
</script>
</body>
</html>
"##;
