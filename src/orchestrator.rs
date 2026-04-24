//! Multi-strategy orchestrator with adaptive capital allocation.
//!
//! Runs all strategies simultaneously, tracks per-strategy performance, and
//! reallocates capital using a multi-armed bandit (softmax over EMA-smoothed
//! risk-adjusted returns). Regime detection biases allocations towards
//! strategies that historically perform well in the current market state.

use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use crate::persistence::Db;
use crate::regime::Regime;
use crate::strategy::performance::StrategyPerformance;
use crate::strategy::{FillEvent, MarketSnapshot, Strategy, StrategyAction};

#[derive(Debug, Clone, Deserialize)]
pub struct OrchestratorConfig {
    #[serde(default = "default_ema_decay")]
    pub ema_decay: f64,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_min_allocation")]
    pub min_allocation: f64,
    #[serde(default = "default_rebalance_interval")]
    pub rebalance_interval: u64,
    #[serde(default = "default_min_trades_for_prior_update")]
    pub min_trades_for_prior_update: u64,
    #[serde(default = "default_strategy_max_drawdown")]
    pub strategy_max_drawdown: f64,
    #[serde(default = "default_strategy_cooldown_ticks")]
    pub strategy_cooldown_ticks: u64,
    #[serde(default = "default_strategies")]
    pub strategies: Vec<String>,
    #[serde(default)]
    pub dca: DcaOrchCfg,
    #[serde(default)]
    pub momentum: MomentumOrchCfg,
    #[serde(default)]
    pub grid: GridOrchCfg,
    #[serde(default)]
    pub pairs: PairsOrchCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DcaOrchCfg {
    #[serde(default = "default_dca_amount")]
    pub buy_amount_quote: f64,
    #[serde(default = "default_dca_interval")]
    pub interval_ticks: u64,
    #[serde(default = "default_true")]
    pub use_sma_filter: bool,
    #[serde(default = "default_sma_period")]
    pub sma_period: usize,
    #[serde(default = "default_dca_take_profit")]
    pub take_profit_pct: f64,
    #[serde(default = "default_dca_trailing_stop")]
    pub trailing_stop_pct: f64,
}

impl Default for DcaOrchCfg {
    fn default() -> Self {
        Self {
            buy_amount_quote: default_dca_amount(),
            interval_ticks: default_dca_interval(),
            use_sma_filter: true,
            sma_period: default_sma_period(),
            take_profit_pct: default_dca_take_profit(),
            trailing_stop_pct: default_dca_trailing_stop(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MomentumOrchCfg {
    #[serde(default = "default_mom_position_size")]
    pub position_size_quote: f64,
    #[serde(default = "default_mom_cooldown")]
    pub cooldown_ticks: u64,
}

impl Default for MomentumOrchCfg {
    fn default() -> Self {
        Self {
            position_size_quote: default_mom_position_size(),
            cooldown_ticks: default_mom_cooldown(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GridOrchCfg {
    #[serde(default = "default_grid_lower")]
    pub lower_price: f64,
    #[serde(default = "default_grid_upper")]
    pub upper_price: f64,
    #[serde(default = "default_grid_levels")]
    pub levels: usize,
    #[serde(default = "default_grid_qty")]
    pub qty_per_level: f64,
}

impl Default for GridOrchCfg {
    fn default() -> Self {
        Self {
            lower_price: default_grid_lower(),
            upper_price: default_grid_upper(),
            levels: default_grid_levels(),
            qty_per_level: default_grid_qty(),
        }
    }
}

fn default_grid_lower() -> f64 {
    60000.0
}
fn default_grid_upper() -> f64 {
    90000.0
}
fn default_grid_levels() -> usize {
    20
}
fn default_grid_qty() -> f64 {
    0.001
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairsOrchCfg {
    #[serde(default = "default_pairs_symbol_a")]
    pub symbol_a: String,
    #[serde(default = "default_pairs_symbol_b")]
    pub symbol_b: String,
    #[serde(default = "default_pairs_lookback")]
    pub lookback: usize,
    #[serde(default = "default_pairs_entry_z")]
    pub entry_z_score: f64,
    #[serde(default = "default_pairs_exit_z")]
    pub exit_z_score: f64,
    #[serde(default = "default_pairs_stop_z")]
    pub stop_z_score: f64,
    #[serde(default = "default_pairs_position_size")]
    pub position_size_quote: f64,
    #[serde(default = "default_pairs_min_correlation")]
    pub min_correlation: f64,
    #[serde(default = "default_pairs_hedge_ratio")]
    pub hedge_ratio: f64,
}

impl Default for PairsOrchCfg {
    fn default() -> Self {
        Self {
            symbol_a: default_pairs_symbol_a(),
            symbol_b: default_pairs_symbol_b(),
            lookback: default_pairs_lookback(),
            entry_z_score: default_pairs_entry_z(),
            exit_z_score: default_pairs_exit_z(),
            stop_z_score: default_pairs_stop_z(),
            position_size_quote: default_pairs_position_size(),
            min_correlation: default_pairs_min_correlation(),
            hedge_ratio: default_pairs_hedge_ratio(),
        }
    }
}

fn default_pairs_symbol_a() -> String {
    "ETHUSDT".to_string()
}
fn default_pairs_symbol_b() -> String {
    "BTCUSDT".to_string()
}
fn default_pairs_lookback() -> usize {
    100
}
fn default_pairs_entry_z() -> f64 {
    2.0
}
fn default_pairs_exit_z() -> f64 {
    0.5
}
fn default_pairs_stop_z() -> f64 {
    3.5
}
fn default_pairs_position_size() -> f64 {
    1000.0
}
fn default_pairs_min_correlation() -> f64 {
    0.7
}
fn default_pairs_hedge_ratio() -> f64 {
    1.0
}

fn default_strategies() -> Vec<String> {
    vec!["dca".to_string(), "momentum".to_string()]
}
fn default_dca_amount() -> f64 {
    100.0
}
fn default_dca_interval() -> u64 {
    10
}
fn default_true() -> bool {
    true
}
fn default_sma_period() -> usize {
    200
}
fn default_dca_take_profit() -> f64 {
    0.05
}
fn default_dca_trailing_stop() -> f64 {
    0.03
}
fn default_mom_position_size() -> f64 {
    1000.0
}
fn default_mom_cooldown() -> u64 {
    5
}

fn default_ema_decay() -> f64 {
    0.9
}
fn default_temperature() -> f64 {
    1.0
}
fn default_min_allocation() -> f64 {
    0.05
}
fn default_rebalance_interval() -> u64 {
    100
}
fn default_min_trades_for_prior_update() -> u64 {
    10
}
fn default_strategy_max_drawdown() -> f64 {
    0.15
}
fn default_strategy_cooldown_ticks() -> u64 {
    200
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            ema_decay: default_ema_decay(),
            temperature: default_temperature(),
            min_allocation: default_min_allocation(),
            rebalance_interval: default_rebalance_interval(),
            min_trades_for_prior_update: default_min_trades_for_prior_update(),
            strategy_max_drawdown: default_strategy_max_drawdown(),
            strategy_cooldown_ticks: default_strategy_cooldown_ticks(),
            strategies: default_strategies(),
            dca: DcaOrchCfg::default(),
            momentum: MomentumOrchCfg::default(),
            grid: GridOrchCfg::default(),
            pairs: PairsOrchCfg::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaggedAction {
    pub strategy_name: String,
    pub action: StrategyAction,
}

pub struct StrategySlot {
    pub strategy: Box<dyn Strategy>,
    pub performance: StrategyPerformance,
    pub allocation: f64,
    pub enabled: bool,
    pub disabled_at_tick: Option<u64>,
}

pub struct Orchestrator {
    slots: Vec<StrategySlot>,
    config: OrchestratorConfig,
    tick_count: u64,
    current_regime: Option<Regime>,
    regime_priors: HashMap<String, f64>,
}

impl Orchestrator {
    pub fn new(strategies: Vec<Box<dyn Strategy>>, config: OrchestratorConfig) -> Self {
        let n = strategies.len();
        let equal_alloc = if n > 0 { 1.0 / n as f64 } else { 0.0 };

        let slots = strategies
            .into_iter()
            .map(|s| {
                let name = s.name().to_string();
                StrategySlot {
                    strategy: s,
                    performance: StrategyPerformance::new(name),
                    allocation: equal_alloc,
                    enabled: true,
                    disabled_at_tick: None,
                }
            })
            .collect();

        let mut orch = Self {
            slots,
            config,
            tick_count: 0,
            current_regime: None,
            regime_priors: HashMap::new(),
        };
        orch.init_regime_priors();
        orch
    }

    fn init_regime_priors(&mut self) {
        let priors = [
            ("RANGING:grid", 2.0),
            ("TRENDING_UP:grid", 0.5),
            ("TRENDING_DOWN:grid", 0.3),
            ("VOLATILE:grid", 0.4),
            ("RANGING:dca", 1.0),
            ("TRENDING_UP:dca", 1.5),
            ("TRENDING_DOWN:dca", 0.8),
            ("VOLATILE:dca", 0.7),
            ("RANGING:momentum", 0.5),
            ("TRENDING_UP:momentum", 2.0),
            ("TRENDING_DOWN:momentum", 1.8),
            ("VOLATILE:momentum", 1.0),
            ("RANGING:pairs", 1.8),
            ("TRENDING_UP:pairs", 0.7),
            ("TRENDING_DOWN:pairs", 0.7),
            ("VOLATILE:pairs", 0.5),
            ("RANGING:funding_arb", 1.2),
            ("TRENDING_UP:funding_arb", 1.2),
            ("TRENDING_DOWN:funding_arb", 1.0),
            ("VOLATILE:funding_arb", 0.6),
        ];

        for (key, weight) in priors {
            self.regime_priors.insert(key.to_string(), weight);
        }
    }

    fn prior_key(regime: &Regime, strategy: &str) -> String {
        format!("{}:{}", regime, strategy)
    }

    pub fn on_tick(
        &mut self,
        snapshot: &MarketSnapshot,
        fills: &[FillEvent],
        regime: Option<Regime>,
    ) -> Result<Vec<TaggedAction>> {
        self.current_regime = regime;
        self.tick_count += 1;

        let mut all_actions = Vec::new();

        for slot in &mut self.slots {
            if !slot.enabled {
                continue;
            }

            let actions = slot.strategy.on_tick(snapshot, fills)?;

            for action in actions {
                let scaled = scale_action(action, slot.allocation);
                all_actions.push(TaggedAction {
                    strategy_name: slot.performance.strategy_name.clone(),
                    action: scaled,
                });
            }
        }

        if self.config.rebalance_interval > 0
            && self.tick_count % self.config.rebalance_interval == 0
            && self.tick_count > 0
        {
            self.check_strategy_health();
            self.rebalance();
            self.try_tune_params(self.tick_count);
        }

        Ok(all_actions)
    }

    pub fn on_start(&mut self, snapshot: &MarketSnapshot) -> Result<Vec<TaggedAction>> {
        let mut all_actions = Vec::new();

        for slot in &mut self.slots {
            if !slot.enabled {
                continue;
            }
            let actions = slot.strategy.on_start(snapshot)?;
            for action in actions {
                let scaled = scale_action(action, slot.allocation);
                all_actions.push(TaggedAction {
                    strategy_name: slot.performance.strategy_name.clone(),
                    action: scaled,
                });
            }
        }

        Ok(all_actions)
    }

    pub fn on_shutdown(&mut self) -> Result<Vec<TaggedAction>> {
        let mut all_actions = Vec::new();

        for slot in &mut self.slots {
            let actions = slot.strategy.on_shutdown()?;
            for action in actions {
                all_actions.push(TaggedAction {
                    strategy_name: slot.performance.strategy_name.clone(),
                    action,
                });
            }
        }

        Ok(all_actions)
    }

    pub fn record_fill(&mut self, strategy_name: &str, pnl: f64) {
        let regime_str = self.current_regime.map(|r| r.to_string());

        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| s.performance.strategy_name == strategy_name)
        {
            slot.performance
                .record_trade(pnl, regime_str.as_deref(), self.config.ema_decay);
        }
    }

    pub fn check_strategy_health(&mut self) {
        let mut changed = false;

        for slot in &mut self.slots {
            if slot.enabled {
                // Disable strategies with excessive drawdown (only after enough trades)
                if slot.performance.trades >= 10
                    && slot.performance.max_drawdown > self.config.strategy_max_drawdown
                {
                    tracing::warn!(
                        strategy = %slot.performance.strategy_name,
                        drawdown_pct = format!("{:.1}", slot.performance.max_drawdown * 100.0),
                        threshold_pct = format!("{:.1}", self.config.strategy_max_drawdown * 100.0),
                        trades = slot.performance.trades,
                        "disabling strategy due to excessive drawdown",
                    );
                    slot.enabled = false;
                    slot.allocation = 0.0;
                    slot.disabled_at_tick = Some(self.tick_count);
                    changed = true;
                }
            } else if let Some(disabled_tick) = slot.disabled_at_tick {
                // Re-enable strategies after cooldown period
                if self.tick_count - disabled_tick >= self.config.strategy_cooldown_ticks {
                    tracing::info!(
                        strategy = %slot.performance.strategy_name,
                        cooldown_ticks = self.config.strategy_cooldown_ticks,
                        "re-enabling strategy after cooldown",
                    );
                    slot.enabled = true;
                    slot.allocation = self.config.min_allocation;
                    slot.disabled_at_tick = None;
                    changed = true;
                }
            }
        }

        if changed {
            self.rebalance();
        }
    }

    pub fn rebalance(&mut self) {
        let active: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.enabled)
            .map(|(i, _)| i)
            .collect();
        let n = active.len();
        if n == 0 {
            return;
        }

        let rewards: Vec<(usize, f64)> = active
            .iter()
            .map(|&i| {
                let slot = &self.slots[i];
                let base_reward = slot.performance.reward_signal();
                // Additive prior bias: ln(prior) acts as a constant offset in
                // the softmax, so regime priors shift allocations even when all
                // strategies have zero performance data.
                let regime_bias = self
                    .current_regime
                    .map(|r| {
                        let key = Self::prior_key(&r, &slot.performance.strategy_name);
                        self.regime_priors.get(&key).unwrap_or(&1.0).ln()
                    })
                    .unwrap_or(0.0);
                (i, base_reward + regime_bias * self.config.temperature)
            })
            .collect();

        let max_r = rewards
            .iter()
            .map(|(_, r)| *r)
            .fold(f64::NEG_INFINITY, f64::max);
        let exp_sum: f64 = rewards
            .iter()
            .map(|(_, r)| ((r - max_r) / self.config.temperature).exp())
            .sum();

        if exp_sum <= 0.0 || !exp_sum.is_finite() {
            let equal = 1.0 / n as f64;
            for &i in &active {
                self.slots[i].allocation = equal;
            }
            return;
        }

        let raw_weights: Vec<(usize, f64)> = rewards
            .iter()
            .map(|(i, r)| {
                let w = ((r - max_r) / self.config.temperature).exp() / exp_sum;
                (*i, w)
            })
            .collect();

        let floor = self.config.min_allocation;
        let total_floor = floor * n as f64;
        if total_floor < 1.0 {
            let remaining = 1.0 - total_floor;
            let raw_sum: f64 = raw_weights.iter().map(|(_, w)| *w).sum();
            for &(i, w) in &raw_weights {
                self.slots[i].allocation = floor + (w / raw_sum) * remaining;
            }
        } else {
            let equal = 1.0 / n as f64;
            for &i in &active {
                self.slots[i].allocation = equal;
            }
        }

        tracing::info!(
            tick = self.tick_count,
            regime = ?self.current_regime,
            "orchestrator rebalanced allocations",
        );
        for &i in &active {
            let slot = &self.slots[i];
            tracing::info!(
                strategy = %slot.performance.strategy_name,
                allocation_pct = format!("{:.1}", slot.allocation * 100.0),
                pnl = format!("{:.2}", slot.performance.total_pnl),
                trades = slot.performance.trades,
                win_rate_pct = format!("{:.1}", slot.performance.win_rate() * 100.0),
                ema_reward = format!("{:.4}", slot.performance.ema_reward),
            );
        }
    }

    pub fn allocations(&self) -> HashMap<String, f64> {
        self.slots
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.performance.strategy_name.clone(), s.allocation))
            .collect()
    }

    pub fn performance(&self, name: &str) -> Option<&StrategyPerformance> {
        self.slots
            .iter()
            .find(|s| s.performance.strategy_name == name)
            .map(|s| &s.performance)
    }

    pub fn all_performances(&self) -> Vec<&StrategyPerformance> {
        self.slots.iter().map(|s| &s.performance).collect()
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|s| s.enabled).count()
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn current_regime(&self) -> Option<Regime> {
        self.current_regime
    }

    pub fn save_state(&self, db: &Db) -> Result<()> {
        for slot in &self.slots {
            let key = format!("orch_perf_{}", slot.performance.strategy_name);
            db.set_state(&key, &slot.performance.to_json())?;

            // Save internal strategy state (price history, indicators, etc.)
            let state_key = format!("orch_strategy_state_{}", slot.performance.strategy_name);
            if let Some(json) = slot.strategy.save_state() {
                db.set_state(&state_key, &json)?;
            }
        }

        let allocs = self.allocations();
        let allocs_json = serde_json::to_string(&allocs).unwrap_or_default();
        db.set_state("orch_allocations", &allocs_json)?;

        let priors_json = serde_json::to_string(&self.regime_priors).unwrap_or_default();
        db.set_state("orch_regime_priors", &priors_json)?;

        Ok(())
    }

    pub fn load_state(&mut self, db: &Db) -> Result<()> {
        for slot in &mut self.slots {
            let key = format!("orch_perf_{}", slot.performance.strategy_name);
            if let Some(json) = db.get_state(&key)? {
                if let Some(perf) = StrategyPerformance::from_json(&json) {
                    slot.performance = perf;
                }
            }

            // Restore internal strategy state
            let state_key = format!("orch_strategy_state_{}", slot.performance.strategy_name);
            if let Some(json) = db.get_state(&state_key)? {
                slot.strategy.load_state(&json);
            }
        }

        if let Some(json) = db.get_state("orch_allocations")? {
            if let Ok(allocs) = serde_json::from_str::<HashMap<String, f64>>(&json) {
                for slot in &mut self.slots {
                    if let Some(&alloc) = allocs.get(&slot.performance.strategy_name) {
                        slot.allocation = alloc;
                    }
                }
            }
        }

        if let Some(json) = db.get_state("orch_regime_priors")? {
            if let Ok(priors) = serde_json::from_str::<HashMap<String, f64>>(&json) {
                self.regime_priors = priors;
            }
        }

        Ok(())
    }

    pub fn try_tune_params(&mut self, tick: u64) {
        for slot in &mut self.slots {
            if !slot.enabled || slot.performance.trades < 10 {
                continue;
            }

            let recent_declining = slot.performance.ema_reward < 0.0
                || (slot.performance.trades > 20 && slot.performance.win_rate() < 0.35);

            if !recent_declining {
                continue;
            }

            let current = slot.strategy.tunable_params();
            if current.is_empty() {
                continue;
            }

            let mut adjusted = HashMap::new();
            let seed = tick
                .wrapping_mul(6364136223846793005)
                .wrapping_add(slot.performance.trades);
            let keys: Vec<String> = current.keys().cloned().collect();
            let key_idx = (seed as usize) % keys.len();
            let key = &keys[key_idx];
            let val = current[key];

            let direction = if (seed >> 16) % 2 == 0 { 1.05 } else { 0.95 };
            adjusted.insert(key.clone(), val * direction);

            slot.strategy.apply_params(&adjusted);
            tracing::info!(
                strategy = %slot.performance.strategy_name,
                param = %key,
                old_val = format!("{:.4}", val),
                new_val = format!("{:.4}", val * direction),
                "auto-tuned underperforming strategy",
            );
        }
    }

    pub fn update_regime_priors(&mut self) {
        let regimes = ["RANGING", "TRENDING_UP", "TRENDING_DOWN", "VOLATILE"];
        let min_trades = self.config.min_trades_for_prior_update;

        for slot in &self.slots {
            for regime_str in &regimes {
                let key = format!("{}:{}", regime_str, slot.performance.strategy_name);
                let rt = slot.performance.regime_trades(regime_str);
                if rt < min_trades {
                    continue;
                }

                let regime_wr = slot.performance.regime_win_rate(regime_str);
                let regime_pnl = slot.performance.regime_pnl(regime_str);

                let current = *self.regime_priors.get(&key).unwrap_or(&1.0);
                let signal = if regime_pnl > 0.0 {
                    1.0 + regime_wr.clamp(0.0, 1.0)
                } else {
                    (1.0 - regime_wr.clamp(0.0, 1.0)).max(0.2)
                };
                let updated = current * 0.9 + signal * 0.1;
                self.regime_priors.insert(key, updated.clamp(0.1, 3.0));
            }
        }
    }
}

fn scale_action(action: StrategyAction, allocation: f64) -> StrategyAction {
    match action {
        StrategyAction::PlaceOrder {
            symbol,
            side,
            price,
            qty,
            order_type,
        } => StrategyAction::PlaceOrder {
            symbol,
            side,
            price,
            qty: qty * allocation,
            order_type,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::OrderType;
    use crate::types::Side;

    struct MockStrategy {
        name: &'static str,
        action: StrategyAction,
    }

    impl Strategy for MockStrategy {
        fn name(&self) -> &'static str {
            self.name
        }
        fn on_start(&mut self, _: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
            Ok(vec![])
        }
        fn on_tick(&mut self, _: &MarketSnapshot, _: &[FillEvent]) -> Result<Vec<StrategyAction>> {
            Ok(vec![self.action.clone()])
        }
        fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
            Ok(vec![])
        }
    }

    fn snap() -> MarketSnapshot {
        MarketSnapshot {
            symbol: "BTCUSDT".into(),
            bid: 49999.0,
            ask: 50001.0,
            last: 50000.0,
            volume_24h: 1000.0,
        }
    }

    fn make_strategies() -> Vec<Box<dyn Strategy>> {
        vec![
            Box::new(MockStrategy {
                name: "grid",
                action: StrategyAction::PlaceOrder {
                    symbol: "BTCUSDT".into(),
                    side: Side::Buy,
                    price: 50000.0,
                    qty: 0.01,
                    order_type: OrderType::Limit,
                },
            }),
            Box::new(MockStrategy {
                name: "momentum",
                action: StrategyAction::PlaceOrder {
                    symbol: "BTCUSDT".into(),
                    side: Side::Buy,
                    price: 50000.0,
                    qty: 0.02,
                    order_type: OrderType::Market,
                },
            }),
        ]
    }

    #[test]
    fn equal_allocation_on_start() {
        let orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        let allocs = orch.allocations();
        assert!((allocs["grid"] - 0.5).abs() < 1e-9);
        assert!((allocs["momentum"] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tick_scales_qty_by_allocation() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        let actions = orch.on_tick(&snap(), &[], None).unwrap();

        for ta in &actions {
            if let StrategyAction::PlaceOrder { qty, .. } = &ta.action {
                if ta.strategy_name == "grid" {
                    assert!(
                        (qty - 0.005).abs() < 1e-9,
                        "grid qty should be 0.01 * 0.5 = 0.005"
                    );
                } else if ta.strategy_name == "momentum" {
                    assert!(
                        (qty - 0.01).abs() < 1e-9,
                        "momentum qty should be 0.02 * 0.5 = 0.01"
                    );
                }
            }
        }
    }

    #[test]
    fn record_fill_updates_performance() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        orch.record_fill("grid", 15.0);
        orch.record_fill("grid", -3.0);
        orch.record_fill("momentum", 20.0);

        let grid_perf = orch.performance("grid").unwrap();
        assert_eq!(grid_perf.trades, 2);
        assert!((grid_perf.total_pnl - 12.0).abs() < 1e-9);

        let mom_perf = orch.performance("momentum").unwrap();
        assert_eq!(mom_perf.trades, 1);
        assert!((mom_perf.total_pnl - 20.0).abs() < 1e-9);
    }

    #[test]
    fn rebalance_shifts_allocation_to_winner() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                temperature: 0.5, // low temp = more exploitation
                ..Default::default()
            },
        );

        // Grid wins consistently
        for _ in 0..20 {
            orch.record_fill("grid", 10.0);
            orch.record_fill("momentum", -2.0);
        }

        orch.rebalance();
        let allocs = orch.allocations();
        assert!(
            allocs["grid"] > allocs["momentum"],
            "grid should get more allocation: grid={:.3} mom={:.3}",
            allocs["grid"],
            allocs["momentum"]
        );
    }

    #[test]
    fn rebalance_respects_min_allocation() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                min_allocation: 0.1,
                temperature: 0.1, // very exploitative
                ..Default::default()
            },
        );

        for _ in 0..50 {
            orch.record_fill("grid", 100.0);
            orch.record_fill("momentum", -50.0);
        }

        orch.rebalance();
        let allocs = orch.allocations();
        assert!(
            allocs["momentum"] >= 0.1,
            "loser should still get min allocation: {:.3}",
            allocs["momentum"]
        );
    }

    #[test]
    fn regime_bias_affects_allocation() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                temperature: 1.0,
                ..Default::default()
            },
        );

        // No performance data — allocations driven purely by regime priors
        orch.current_regime = Some(Regime::Ranging);
        orch.rebalance();
        let allocs = orch.allocations();
        // In RANGING, grid prior=2.0, momentum prior=0.5
        assert!(
            allocs["grid"] > allocs["momentum"],
            "grid should dominate in RANGING: grid={:.3} mom={:.3}",
            allocs["grid"],
            allocs["momentum"]
        );
    }

    #[test]
    fn regime_trending_favors_momentum() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                temperature: 1.0,
                ..Default::default()
            },
        );

        orch.current_regime = Some(Regime::TrendingUp);
        orch.rebalance();
        let allocs = orch.allocations();
        // In TRENDING_UP, grid prior=0.5, momentum prior=2.0
        assert!(
            allocs["momentum"] > allocs["grid"],
            "momentum should dominate in TRENDING_UP: mom={:.3} grid={:.3}",
            allocs["momentum"],
            allocs["grid"]
        );
    }

    #[test]
    fn auto_rebalance_on_interval() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                rebalance_interval: 5,
                temperature: 0.5,
                ..Default::default()
            },
        );

        // Feed some wins to grid
        for _ in 0..10 {
            orch.record_fill("grid", 10.0);
            orch.record_fill("momentum", -1.0);
        }

        // Tick 5 times — rebalance should trigger on tick 5
        for _ in 0..5 {
            let _ = orch.on_tick(&snap(), &[], None).unwrap();
        }

        let allocs = orch.allocations();
        assert!(
            allocs["grid"] > allocs["momentum"],
            "after auto-rebalance, grid should lead"
        );
    }

    #[test]
    fn disabled_strategy_skipped() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        orch.slots[1].enabled = false;

        let actions = orch.on_tick(&snap(), &[], None).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].strategy_name, "grid");
    }

    #[test]
    fn scale_action_preserves_non_order() {
        let action = StrategyAction::None;
        let scaled = scale_action(action, 0.3);
        assert!(matches!(scaled, StrategyAction::None));

        let cancel = StrategyAction::CancelOrder {
            order_id: "x".into(),
        };
        let scaled = scale_action(cancel, 0.3);
        assert!(matches!(scaled, StrategyAction::CancelOrder { .. }));
    }

    #[test]
    fn active_count() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        assert_eq!(orch.active_count(), 2);
        orch.slots[0].enabled = false;
        assert_eq!(orch.active_count(), 1);
    }

    #[test]
    fn persistence_round_trip() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        orch.record_fill("grid", 25.0);
        orch.record_fill("momentum", -5.0);

        let db = crate::persistence::Db::open_in_memory().unwrap();
        orch.save_state(&db).unwrap();

        let mut orch2 = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        orch2.load_state(&db).unwrap();

        let grid = orch2.performance("grid").unwrap();
        assert_eq!(grid.trades, 1);
        assert!((grid.total_pnl - 25.0).abs() < 1e-9);
    }

    #[test]
    fn update_regime_priors_adjusts_weights() {
        let mut orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());

        // Feed enough regime-tagged trades to trigger prior update
        for _ in 0..15 {
            orch.current_regime = Some(Regime::Ranging);
            orch.record_fill("grid", 10.0);
        }

        let key = "RANGING:grid".to_string();
        let before = *orch.regime_priors.get(&key).unwrap();
        orch.update_regime_priors();
        let after = *orch.regime_priors.get(&key).unwrap();

        // Grid winning in RANGING should push prior up (or keep it high)
        assert!(
            after >= before * 0.9,
            "prior should not drop significantly when strategy is winning"
        );
    }

    #[test]
    fn all_performances_returns_all() {
        let orch = Orchestrator::new(make_strategies(), OrchestratorConfig::default());
        assert_eq!(orch.all_performances().len(), 2);
    }

    #[test]
    fn on_start_scales_actions() {
        struct StartStrategy;
        impl Strategy for StartStrategy {
            fn name(&self) -> &'static str {
                "starter"
            }
            fn on_start(&mut self, _: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
                Ok(vec![StrategyAction::PlaceOrder {
                    symbol: "BTC".into(),
                    side: Side::Buy,
                    price: 100.0,
                    qty: 1.0,
                    order_type: OrderType::Market,
                }])
            }
            fn on_tick(
                &mut self,
                _: &MarketSnapshot,
                _: &[FillEvent],
            ) -> Result<Vec<StrategyAction>> {
                Ok(vec![])
            }
            fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
                Ok(vec![])
            }
        }

        let strategies: Vec<Box<dyn Strategy>> = vec![Box::new(StartStrategy)];
        let mut orch = Orchestrator::new(strategies, OrchestratorConfig::default());
        let actions = orch.on_start(&snap()).unwrap();
        assert_eq!(actions.len(), 1);
        if let StrategyAction::PlaceOrder { qty, .. } = &actions[0].action {
            assert!((qty - 1.0).abs() < 1e-9, "single strategy gets 100%");
        }
    }

    #[test]
    fn on_shutdown_collects_all() {
        struct ShutdownStrategy;
        impl Strategy for ShutdownStrategy {
            fn name(&self) -> &'static str {
                "closer"
            }
            fn on_start(&mut self, _: &MarketSnapshot) -> Result<Vec<StrategyAction>> {
                Ok(vec![])
            }
            fn on_tick(
                &mut self,
                _: &MarketSnapshot,
                _: &[FillEvent],
            ) -> Result<Vec<StrategyAction>> {
                Ok(vec![])
            }
            fn on_shutdown(&mut self) -> Result<Vec<StrategyAction>> {
                Ok(vec![StrategyAction::CancelAll {
                    symbol: "BTC".into(),
                }])
            }
        }

        let strategies: Vec<Box<dyn Strategy>> =
            vec![Box::new(ShutdownStrategy), Box::new(ShutdownStrategy)];
        let mut orch = Orchestrator::new(strategies, OrchestratorConfig::default());
        let actions = orch.on_shutdown().unwrap();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn empty_orchestrator_handles_gracefully() {
        let mut orch = Orchestrator::new(vec![], OrchestratorConfig::default());
        let actions = orch.on_tick(&snap(), &[], None).unwrap();
        assert!(actions.is_empty());
        orch.rebalance(); // should not panic
        assert_eq!(orch.active_count(), 0);
    }

    #[test]
    fn high_temperature_gives_near_uniform_allocation() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                temperature: 10000.0, // very high = nearly uniform
                ..Default::default()
            },
        );

        for _ in 0..20 {
            orch.record_fill("grid", 100.0);
            orch.record_fill("momentum", -50.0);
        }

        orch.rebalance();
        let allocs = orch.allocations();
        let diff = (allocs["grid"] - allocs["momentum"]).abs();
        assert!(
            diff < 0.15,
            "high temperature should keep allocations close: diff={:.3}",
            diff
        );
    }

    #[test]
    fn strategy_disabled_on_excessive_drawdown() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                strategy_max_drawdown: 0.10, // 10% threshold
                ..Default::default()
            },
        );

        // Build up a peak with winning trades, then lose enough to exceed 10% drawdown
        for _ in 0..5 {
            orch.record_fill("grid", 20.0); // peak_pnl = 100
        }
        for _ in 0..6 {
            orch.record_fill("grid", -5.0); // total_pnl = 70, dd = 30/100 = 30%
        }
        // grid now has 11 trades and 30% drawdown (> 10% threshold)

        assert!(
            orch.slots[0].enabled,
            "grid should still be enabled before health check"
        );
        orch.check_strategy_health();
        assert!(
            !orch.slots[0].enabled,
            "grid should be disabled after health check"
        );
        assert!(
            orch.slots[0].disabled_at_tick.is_some(),
            "disabled_at_tick should be set"
        );
        assert!(
            (orch.slots[0].allocation - 0.0).abs() < 1e-9,
            "disabled strategy should have 0 allocation",
        );
    }

    #[test]
    fn strategy_re_enabled_after_cooldown() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                strategy_max_drawdown: 0.10,
                strategy_cooldown_ticks: 50,
                rebalance_interval: 1, // rebalance every tick so health check runs
                ..Default::default()
            },
        );

        // Build drawdown on grid to trigger disable
        for _ in 0..5 {
            orch.record_fill("grid", 20.0);
        }
        for _ in 0..6 {
            orch.record_fill("grid", -5.0);
        }

        // Advance ticks to trigger the health check that disables the strategy
        let _ = orch.on_tick(&snap(), &[], None).unwrap();
        assert!(!orch.slots[0].enabled, "grid should be disabled");
        let _disabled_tick = orch.slots[0].disabled_at_tick.unwrap();

        // Advance ticks past cooldown
        for _ in 0..50 {
            let _ = orch.on_tick(&snap(), &[], None).unwrap();
        }

        assert!(
            orch.slots[0].enabled,
            "grid should be re-enabled after cooldown"
        );
        assert!(
            orch.slots[0].disabled_at_tick.is_none(),
            "disabled_at_tick should be cleared",
        );
        assert!(
            orch.slots[0].allocation >= orch.config.min_allocation - 1e-9,
            "re-enabled strategy should have at least min_allocation: got {:.4}",
            orch.slots[0].allocation,
        );
    }

    #[test]
    fn healthy_strategy_stays_enabled() {
        let mut orch = Orchestrator::new(
            make_strategies(),
            OrchestratorConfig {
                strategy_max_drawdown: 0.15, // 15% threshold
                ..Default::default()
            },
        );

        // Give grid enough trades with mild drawdown (below threshold)
        for _ in 0..8 {
            orch.record_fill("grid", 10.0); // peak = 80
        }
        for _ in 0..3 {
            orch.record_fill("grid", -3.0); // total = 71, dd = 9/80 = 11.25%
        }
        // 11 trades, 11.25% drawdown < 15% threshold

        orch.check_strategy_health();
        assert!(
            orch.slots[0].enabled,
            "grid should remain enabled with drawdown below threshold"
        );
        assert!(orch.slots[0].disabled_at_tick.is_none());
    }
}
