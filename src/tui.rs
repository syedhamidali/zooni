//! Terminal UI dashboard using ratatui.
//!
//! Shows live grid state, P&L, fills, and risk metrics.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
};
use tokio::sync::Mutex;

use crate::config::BotConfig;
use crate::venue::Venue;

/// Shared state between the bot and the TUI.
pub struct TuiState {
    pub symbol: String,
    pub grid_lower: f64,
    pub grid_upper: f64,
    pub grid_levels: usize,
    pub current_price: f64,
    pub equity: f64,
    pub daily_pnl: f64,
    pub total_pnl: f64,
    pub total_fills: u64,
    pub open_orders: u64,
    pub dry_run: bool,
    pub uptime_secs: u64,
    pub recent_fills: Vec<FillDisplay>,
    pub grid_state: Vec<GridLevelDisplay>,
    pub last_error: Option<String>,
    pub circuit_breaker_open: bool,
}

#[derive(Clone)]
pub struct FillDisplay {
    pub time: String,
    pub side: String,
    pub price: f64,
    pub qty: f64,
    pub pnl: f64,
}

#[derive(Clone)]
pub struct GridLevelDisplay {
    pub level: usize,
    pub price: f64,
    pub side: String,     // "BUY" / "SELL" / "---"
    pub status: String,   // "OPEN" / "FILLED" / "---"
    pub is_current: bool, // closest to current price
}

impl TuiState {
    pub fn new(cfg: &BotConfig) -> Self {
        Self {
            symbol: cfg.grid.symbol.clone(),
            grid_lower: cfg.grid.lower_price,
            grid_upper: cfg.grid.upper_price,
            grid_levels: cfg.grid.levels as usize,
            current_price: 0.0,
            equity: cfg.risk.starting_equity_quote,
            daily_pnl: 0.0,
            total_pnl: 0.0,
            total_fills: 0,
            open_orders: 0,
            dry_run: cfg.bot.dry_run,
            uptime_secs: 0,
            recent_fills: Vec::new(),
            grid_state: Vec::new(),
            last_error: None,
            circuit_breaker_open: false,
        }
    }
}

/// Run the bot with TUI overlay. The bot writes live data into shared TuiState,
/// and the render loop displays it in the terminal.
pub async fn run_with_tui(cfg: BotConfig, venue: Venue) -> Result<()> {
    let state = Arc::new(Mutex::new(TuiState::new(&cfg)));

    // Start TUI rendering in a separate task
    let tui_state = state.clone();
    let tui_handle = tokio::spawn(async move {
        if let Err(e) = render_loop(tui_state).await {
            eprintln!("TUI error: {e}");
        }
    });

    // Run the bot with shared TUI state — bot updates state each tick
    let bot_state = state.clone();
    let bot_handle = tokio::spawn(async move {
        if let Err(e) = crate::bot::run_with_shared_tui(cfg, venue, bot_state).await {
            eprintln!("Bot error: {e}");
        }
    });

    tokio::select! {
        _ = tui_handle => {}
        _ = bot_handle => {}
    }

    Ok(())
}

async fn render_loop(state: Arc<Mutex<TuiState>>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let tick_rate = Duration::from_millis(500);

    loop {
        let s = state.lock().await;
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // header
                    Constraint::Min(8),    // grid + fills
                    Constraint::Length(3), // footer
                ])
                .split(f.area());

            // Header
            let mode = if s.dry_run { " [DRY-RUN]" } else { "" };
            let header_text = format!(
                " {} | Price: {:.2} | Equity: {:.2} | PnL: {:+.2} | Fills: {} | Open: {}{}",
                s.symbol,
                s.current_price,
                s.equity,
                s.total_pnl,
                s.total_fills,
                s.open_orders,
                mode,
            );
            let header = Paragraph::new(header_text)
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Trading Bot "),
                );
            f.render_widget(header, chunks[0]);

            // Main area: grid on left, fills on right
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);

            // Grid state table
            let grid_rows: Vec<Row> = s
                .grid_state
                .iter()
                .rev()
                .take(20)
                .map(|g| {
                    let style = if g.is_current {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else if g.side == "BUY" {
                        Style::default().fg(Color::Green)
                    } else if g.side == "SELL" {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    Row::new(vec![
                        format!("{:>3}", g.level),
                        format!("{:.2}", g.price),
                        g.side.clone(),
                        g.status.clone(),
                    ])
                    .style(style)
                })
                .collect();
            let grid_table = Table::new(
                grid_rows,
                [
                    Constraint::Length(5),
                    Constraint::Length(14),
                    Constraint::Length(6),
                    Constraint::Length(8),
                ],
            )
            .header(
                Row::new(vec!["Lvl", "Price", "Side", "Status"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(Block::default().borders(Borders::ALL).title(" Grid "));
            f.render_widget(grid_table, main_chunks[0]);

            // Recent fills
            let fill_rows: Vec<Row> = s
                .recent_fills
                .iter()
                .rev()
                .take(15)
                .map(|fill| {
                    let style = if fill.side == "BUY" {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Red)
                    };
                    Row::new(vec![
                        fill.time.clone(),
                        fill.side.clone(),
                        format!("{:.2}", fill.price),
                        format!("{:.6}", fill.qty),
                        format!("{:+.4}", fill.pnl),
                    ])
                    .style(style)
                })
                .collect();
            let fills_table = Table::new(
                fill_rows,
                [
                    Constraint::Length(10),
                    Constraint::Length(5),
                    Constraint::Length(12),
                    Constraint::Length(10),
                    Constraint::Length(10),
                ],
            )
            .header(
                Row::new(vec!["Time", "Side", "Price", "Qty", "PnL"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recent Fills "),
            );
            f.render_widget(fills_table, main_chunks[1]);

            // Footer
            let mut footer_spans = vec![
                Span::styled(
                    " q",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("=quit "),
                Span::styled("daily", Style::default().fg(Color::Cyan)),
                Span::raw(format!(": {:+.2} ", s.daily_pnl)),
            ];
            if s.circuit_breaker_open {
                footer_spans.push(Span::styled(
                    "CIRCUIT BREAKER OPEN",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(ref err) = s.last_error {
                footer_spans.push(Span::styled(
                    format!(" ERR: {}", &err[..err.len().min(50)]),
                    Style::default().fg(Color::Red),
                ));
            }
            let footer = Paragraph::new(Line::from(footer_spans))
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(footer, chunks[2]);
        })?;
        drop(s);

        // Handle input
        if event::poll(tick_rate)?
            && let Event::Key(key) = event::read()?
            && (key.code == KeyCode::Char('q')
                || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}
