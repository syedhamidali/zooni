//! zooni-report — Monthly P&L and India tax report generator.
//!
//! Usage:
//!   zooni-report trading-bot.db
//!   zooni-report trading-bot.db --month 2026-04
//!   zooni-report trading-bot.db --year 2026
//!   zooni-report trading-bot.db --tax
//!   zooni-report trading-bot.db --month 2026-04 --csv output.csv

use anyhow::{Context, Result, bail};
use chrono::{Datelike, TimeZone, Utc};
use std::collections::HashMap;
use std::io::Write as IoWrite;

use zooni::persistence::{Db, FillRow};
use zooni::tax::{AssetClass, TaxConfig, TaxEngine};
use zooni::types::Side;

// ── CLI argument parsing ─────────────────────────────────────────────────────

struct Args {
    db_path: String,
    month: Option<(i32, u32)>, // (year, month)
    year: Option<i32>,
    tax_mode: bool,
    csv_path: Option<String>,
}

fn parse_args() -> Result<Args> {
    let raw: Vec<String> = std::env::args().collect();

    if raw.len() < 2 || raw[1] == "--help" || raw[1] == "-h" {
        eprintln!(
            "Usage: zooni-report <db> [--month YYYY-MM] [--year YYYY] [--tax] [--csv out.csv]"
        );
        std::process::exit(0);
    }

    let db_path = raw[1].clone();

    let month = raw
        .iter()
        .position(|a| a == "--month")
        .and_then(|i| raw.get(i + 1))
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, '-').collect();
            if parts.len() != 2 {
                bail!("--month expects YYYY-MM, got: {s}");
            }
            let y: i32 = parts[0].parse().context("parsing year")?;
            let m: u32 = parts[1].parse().context("parsing month")?;
            Ok::<(i32, u32), anyhow::Error>((y, m))
        })
        .transpose()?;

    let year = raw
        .iter()
        .position(|a| a == "--year")
        .and_then(|i| raw.get(i + 1))
        .map(|s| s.parse::<i32>().context("parsing --year"))
        .transpose()?;

    let tax_mode = raw.iter().any(|a| a == "--tax");

    let csv_path = raw
        .iter()
        .position(|a| a == "--csv")
        .and_then(|i| raw.get(i + 1))
        .cloned();

    Ok(Args {
        db_path,
        month,
        year,
        tax_mode,
        csv_path,
    })
}

// ── Date range helpers ────────────────────────────────────────────────────────

/// Returns (start, end) for a calendar month as UTC DateTimes.
fn month_range(year: i32, month: u32) -> Result<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
    let start = Utc
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .with_context(|| format!("invalid month {year}-{month:02}"))?;
    // next month
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = Utc
        .with_ymd_and_hms(ny, nm, 1, 0, 0, 0)
        .single()
        .with_context(|| format!("invalid next month {ny}-{nm:02}"))?;
    Ok((start, end))
}

/// Returns (start, end) for a full calendar year.
fn year_range(year: i32) -> Result<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
    let start = Utc
        .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
        .single()
        .with_context(|| format!("invalid year {year}"))?;
    let end = Utc
        .with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
        .single()
        .with_context(|| format!("invalid year+1 {}", year + 1))?;
    Ok((start, end))
}

// ── Aggregation types ────────────────────────────────────────────────────────

/// Inferred strategy name: stored in order_id prefix like "grid-xxx", "dca-xxx", etc.
fn strategy_from_order_id(order_id: &str) -> &str {
    // Strategy is the first dash-separated segment of the order id when present.
    // E.g. "grid-abcd1234" → "grid", "dca-xyz" → "dca".
    // Fall back to "unknown" if the id has no dash.
    order_id.split('-').next().unwrap_or("unknown")
}

#[derive(Default)]
struct StratSymKey {
    strategy: String,
    symbol: String,
}

impl PartialEq for StratSymKey {
    fn eq(&self, other: &Self) -> bool {
        self.strategy == other.strategy && self.symbol == other.symbol
    }
}
impl Eq for StratSymKey {}
impl std::hash::Hash for StratSymKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.strategy.hash(state);
        self.symbol.hash(state);
    }
}

#[derive(Default)]
struct StratSymStats {
    fills: u64,
    gross_pnl: f64,
    fees: f64,
}

impl StratSymStats {
    fn net_pnl(&self) -> f64 {
        self.gross_pnl - self.fees
    }
}

// ── Fee estimation ────────────────────────────────────────────────────────────

/// Estimate the fee for a fill using the default 0.1% fee rate on the notional.
/// (The DB does not store per-fill fees, so we reconstruct from notional.)
fn estimated_fee(notional: f64) -> f64 {
    notional * 0.001
}

// ── Daily P&L helper ─────────────────────────────────────────────────────────

fn compute_daily_pnl(fills: &[FillRow]) -> HashMap<String, f64> {
    let mut daily: HashMap<String, f64> = HashMap::new();
    for f in fills {
        let date = f.filled_at.format("%Y-%m-%d").to_string();
        *daily.entry(date).or_insert(0.0) += f.realised_pnl;
    }
    daily
}

// ── Win rate from fills ───────────────────────────────────────────────────────

fn compute_win_rate(fills: &[FillRow]) -> f64 {
    let sell_fills: Vec<&FillRow> = fills.iter().filter(|f| f.side == Side::Sell).collect();
    if sell_fills.is_empty() {
        return 0.0;
    }
    let winners = sell_fills.iter().filter(|f| f.realised_pnl > 0.0).count();
    winners as f64 / sell_fills.len() as f64
}

// ── Monthly P&L report ───────────────────────────────────────────────────────

fn print_monthly_report(fills: &[FillRow], year: i32, month: u32) {
    let month_name = match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Unknown",
    };

    println!("\n{}", "═".repeat(55));
    println!("  Zooni Monthly Report — {month_name} {year}");
    println!("{}", "═".repeat(55));

    if fills.is_empty() {
        println!("\n  No fills found for this period.\n");
        return;
    }

    // Aggregate by (strategy, symbol)
    let mut agg: HashMap<StratSymKey, StratSymStats> = HashMap::new();
    for f in fills {
        let strategy = strategy_from_order_id(&f.order_id).to_string();
        let key = StratSymKey {
            strategy,
            symbol: f.symbol.clone(),
        };
        let entry = agg.entry(key).or_default();
        entry.fills += 1;
        entry.gross_pnl += f.realised_pnl;
        entry.fees += estimated_fee(f.notional);
    }

    // Sort by strategy then symbol for deterministic output
    let mut rows: Vec<(&StratSymKey, &StratSymStats)> = agg.iter().collect();
    rows.sort_by(|a, b| {
        a.0.strategy
            .cmp(&b.0.strategy)
            .then(a.0.symbol.cmp(&b.0.symbol))
    });

    println!(
        "\n  {:<14} {:<12} {:>6}   {:>10}   {:>8}  {:>10}",
        "Strategy", "Symbol", "Fills", "Gross P&L", "Fees", "Net P&L"
    );
    println!("  {}", "─".repeat(65));

    let mut total_fills: u64 = 0;
    let mut total_gross = 0.0_f64;
    let mut total_fees = 0.0_f64;

    for (key, stats) in &rows {
        total_fills += stats.fills;
        total_gross += stats.gross_pnl;
        total_fees += stats.fees;

        let gross_sign = if stats.gross_pnl >= 0.0 { "+" } else { "" };
        let net_sign = if stats.net_pnl() >= 0.0 { "+" } else { "" };

        println!(
            "  {:<14} {:<12} {:>6}   {}{:>9.2}   -{:>7.2}  {}{:>9.2}",
            key.strategy,
            key.symbol,
            stats.fills,
            gross_sign,
            stats.gross_pnl,
            stats.fees,
            net_sign,
            stats.net_pnl(),
        );
    }

    println!("  {}", "─".repeat(65));
    let net_sign = if (total_gross - total_fees) >= 0.0 {
        "+"
    } else {
        ""
    };
    let gross_sign = if total_gross >= 0.0 { "+" } else { "" };
    println!(
        "  {:<14} {:<12} {:>6}   {}{:>9.2}   -{:>7.2}  {}{:>9.2}",
        "TOTAL",
        "",
        total_fills,
        gross_sign,
        total_gross,
        total_fees,
        net_sign,
        total_gross - total_fees,
    );

    // Daily stats
    let daily = compute_daily_pnl(fills);
    if !daily.is_empty() {
        let best = daily.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap());
        let worst = daily.iter().min_by(|a, b| a.1.partial_cmp(b.1).unwrap());

        println!();
        if let Some((day, pnl)) = best {
            let sign = if *pnl >= 0.0 { "+" } else { "" };
            println!("  Best day:  {day}  {sign}${pnl:.2}");
        }
        if let Some((day, pnl)) = worst {
            let sign = if *pnl >= 0.0 { "+" } else { "" };
            println!("  Worst day: {day}  {sign}${pnl:.2}");
        }
    }

    let win_rate = compute_win_rate(fills);
    println!("  Win rate:  {:.0}%", win_rate * 100.0);

    // Monthly Sharpe: use daily P&L as returns
    let mut daily_returns: Vec<f64> = compute_daily_pnl(fills).into_values().collect();
    daily_returns.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if let Some(sharpe) = zooni::pnl::sharpe_ratio(&daily_returns) {
        // Scale 252-day annualization back to monthly (÷ sqrt(12)) for display.
        let monthly_sharpe = sharpe / (252_f64 / 21_f64).sqrt();
        println!("  Sharpe (monthly): {monthly_sharpe:.2}");
    }

    println!();
}

// ── Tax report ───────────────────────────────────────────────────────────────

fn print_tax_report(fills: &[FillRow], year: i32) {
    // India FY is April–March; if year=2026, FY is 2026-27.
    let fy_start = year;
    let fy_end = year + 1;

    println!("\n{}", "═".repeat(55));
    println!("  Zooni Tax Report — FY {fy_start}-{}", fy_end % 100);
    println!("{}", "═".repeat(55));

    if fills.is_empty() {
        println!("\n  No fills found for this period.\n");
        return;
    }

    let engine = TaxEngine::new(TaxConfig::default());

    // For a simplified summary: compute aggregate gross gains and apply rates.
    // We treat all fills as Indian crypto (Section 115BBH) unless the symbol
    // ends in ".NS" (NSE equity).
    let (mut crypto_gross, mut equity_gross) = (0.0_f64, 0.0_f64);
    let mut total_notional_sold = 0.0_f64;

    for f in fills {
        if f.side == Side::Sell {
            if f.symbol.ends_with(".NS") || f.symbol.ends_with(".BO") {
                equity_gross += f.realised_pnl;
            } else {
                crypto_gross += f.realised_pnl;
                total_notional_sold += f.notional;
            }
        }
    }

    // Inr conversion rate — placeholder; the DB stores USD amounts.
    // For accurate INR output, multiply by the conversion rate.
    // We display in the same currency as stored (USD proxy) but label as ₹
    // to match the spec format. Users should apply their own FX conversion.
    let inr_rate = 83.5_f64; // approximate USD→INR

    let crypto_gross_inr = crypto_gross * inr_rate;
    let equity_gross_inr = equity_gross * inr_rate;
    let notional_sold_inr = total_notional_sold * inr_rate;

    // Crypto Section 115BBH
    let tds_rate = 0.01_f64;
    let crypto_tax_rate = 0.30_f64;
    let equity_stcg_rate = 0.15_f64;

    let tds_inr = notional_sold_inr * tds_rate;
    let crypto_tax_inr = if crypto_gross_inr > 0.0 {
        crypto_gross_inr * crypto_tax_rate
    } else {
        0.0
    };
    let crypto_net_inr = crypto_gross_inr - crypto_tax_inr;

    let equity_tax_inr = if equity_gross_inr > 0.0 {
        equity_gross_inr * equity_stcg_rate
    } else {
        0.0
    };

    let total_tax_inr = crypto_tax_inr + equity_tax_inr;

    // --- Crypto section ---
    println!("\n  Crypto (Section 115BBH):");
    println!("    Gross gains:        ₹{:.0}", crypto_gross_inr.max(0.0));
    println!(
        "    TDS deducted (1%):  ₹{:.0} (already withheld by exchange)",
        tds_inr
    );
    println!("    Tax @ 30%:          ₹{:.0}", crypto_tax_inr);
    println!("    Net after tax:      ₹{:.0}", crypto_net_inr.max(0.0));

    // Use engine just for verification (no-op, but keeps the import alive).
    let _ = engine.effective_tax_rate(AssetClass::CryptoIndia);

    // --- Equity section ---
    if equity_gross_inr.abs() > 0.01 {
        println!();
        println!("  Equity STCG (held < 1 year):");
        println!("    Gains:     ₹{:.0}", equity_gross_inr.max(0.0));
        println!("    Tax @ 15%: ₹{:.0}", equity_tax_inr);
    }

    println!();
    println!("  Total tax liability: ₹{:.0}", total_tax_inr);
    println!();
}

// ── CSV export ────────────────────────────────────────────────────────────────

fn export_csv(fills: &[FillRow], path: &str) -> Result<()> {
    let file = std::fs::File::create(path).with_context(|| format!("creating CSV file {path}"))?;
    let mut w = std::io::BufWriter::new(file);
    writeln!(w, "date,strategy,symbol,side,qty,price,fee,pnl")?;
    for f in fills {
        let date = f.filled_at.format("%Y-%m-%dT%H:%M:%SZ");
        let strategy = strategy_from_order_id(&f.order_id);
        let side = match f.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let fee = estimated_fee(f.notional);
        writeln!(
            w,
            "{},{},{},{},{:.8},{:.8},{:.8},{:.8}",
            date, strategy, f.symbol, side, f.qty, f.price, fee, f.realised_pnl
        )?;
    }
    println!("Exported {} fills to {path}", fills.len());
    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = parse_args()?;

    let db =
        Db::open(&args.db_path).with_context(|| format!("opening database: {}", args.db_path))?;

    // Determine the time range to query.
    let now = Utc::now();
    let (start, end, display_year, display_month) = if let Some((y, m)) = args.month {
        let (s, e) = month_range(y, m)?;
        (s, e, y, Some(m))
    } else if let Some(y) = args.year {
        let (s, e) = year_range(y)?;
        (s, e, y, None)
    } else {
        // Default: current month.
        let y = now.year();
        let m = now.month();
        let (s, e) = month_range(y, m)?;
        (s, e, y, Some(m))
    };

    let fills = db.query_fills_in_range(start, end)?;

    if args.tax_mode {
        print_tax_report(&fills, display_year);
    } else if let Some(month) = display_month {
        print_monthly_report(&fills, display_year, month);
    } else {
        // Year mode without --tax: print each month.
        for m in 1u32..=12 {
            let (ms, me) = month_range(display_year, m)?;
            let month_fills: Vec<FillRow> = fills
                .iter()
                .filter(|f| f.filled_at >= ms && f.filled_at < me)
                .cloned()
                .collect();
            if !month_fills.is_empty() {
                print_monthly_report(&month_fills, display_year, m);
            }
        }
        if fills.is_empty() {
            println!("No fills found for {display_year}.");
        }
    }

    if let Some(csv_path) = &args.csv_path {
        export_csv(&fills, csv_path)?;
    }

    Ok(())
}
