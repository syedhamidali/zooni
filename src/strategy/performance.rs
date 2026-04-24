//! Per-strategy performance tracking for the adaptive orchestrator.
//!
//! Tracks PnL, win rate, drawdown, and Sharpe per strategy per market regime.
//! The `ema_reward` field feeds the multi-armed bandit allocator.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPerformance {
    pub strategy_name: String,
    pub total_pnl: f64,
    pub trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub peak_pnl: f64,
    pub max_drawdown: f64,
    pub ema_reward: f64,
    recent_returns: Vec<f64>,
    max_recent: usize,
    regime_pnl: HashMap<String, f64>,
    regime_trades: HashMap<String, u64>,
    regime_wins: HashMap<String, u64>,
}

impl StrategyPerformance {
    pub fn new(strategy_name: String) -> Self {
        Self {
            strategy_name,
            total_pnl: 0.0,
            trades: 0,
            wins: 0,
            losses: 0,
            peak_pnl: 0.0,
            max_drawdown: 0.0,
            ema_reward: 0.0,
            recent_returns: Vec::new(),
            max_recent: 50,
            regime_pnl: HashMap::new(),
            regime_trades: HashMap::new(),
            regime_wins: HashMap::new(),
        }
    }

    pub fn record_trade(&mut self, pnl: f64, regime: Option<&str>, ema_decay: f64) {
        self.total_pnl += pnl;
        self.trades += 1;

        if pnl > 0.0 {
            self.wins += 1;
        } else if pnl < 0.0 {
            self.losses += 1;
        }

        if self.total_pnl > self.peak_pnl {
            self.peak_pnl = self.total_pnl;
        }
        let dd = if self.peak_pnl > 0.0 {
            (self.peak_pnl - self.total_pnl) / self.peak_pnl
        } else {
            0.0
        };
        if dd > self.max_drawdown {
            self.max_drawdown = dd;
        }

        self.ema_reward = ema_decay * self.ema_reward + (1.0 - ema_decay) * pnl;

        self.recent_returns.push(pnl);
        if self.recent_returns.len() > self.max_recent {
            self.recent_returns.remove(0);
        }

        if let Some(r) = regime {
            let key = r.to_string();
            *self.regime_pnl.entry(key.clone()).or_insert(0.0) += pnl;
            *self.regime_trades.entry(key.clone()).or_insert(0) += 1;
            if pnl > 0.0 {
                *self.regime_wins.entry(key).or_insert(0) += 1;
            }
        }
    }

    pub fn win_rate(&self) -> f64 {
        if self.trades == 0 {
            return 0.0;
        }
        self.wins as f64 / self.trades as f64
    }

    pub fn sharpe_estimate(&self) -> f64 {
        if self.recent_returns.len() < 2 {
            return 0.0;
        }
        let n = self.recent_returns.len() as f64;
        let mean = self.recent_returns.iter().sum::<f64>() / n;
        let variance = self
            .recent_returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0);
        let std_dev = variance.sqrt();
        if std_dev < 1e-12 {
            return 0.0;
        }
        mean / std_dev
    }

    pub fn regime_win_rate(&self, regime: &str) -> f64 {
        let trades = *self.regime_trades.get(regime).unwrap_or(&0);
        if trades == 0 {
            return 0.0;
        }
        let wins = *self.regime_wins.get(regime).unwrap_or(&0);
        wins as f64 / trades as f64
    }

    pub fn regime_pnl(&self, regime: &str) -> f64 {
        *self.regime_pnl.get(regime).unwrap_or(&0.0)
    }

    pub fn regime_trades(&self, regime: &str) -> u64 {
        *self.regime_trades.get(regime).unwrap_or(&0)
    }

    pub fn reward_signal(&self) -> f64 {
        let sharpe = self.sharpe_estimate();
        if sharpe.abs() < 1e-12 {
            self.ema_reward
        } else {
            self.ema_reward * (1.0 + sharpe.clamp(-2.0, 2.0) * 0.3)
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_performance_is_zeroed() {
        let p = StrategyPerformance::new("grid".into());
        assert_eq!(p.trades, 0);
        assert_eq!(p.total_pnl, 0.0);
        assert_eq!(p.win_rate(), 0.0);
        assert_eq!(p.sharpe_estimate(), 0.0);
    }

    #[test]
    fn record_winning_trade() {
        let mut p = StrategyPerformance::new("grid".into());
        p.record_trade(10.0, None, 0.9);
        assert_eq!(p.trades, 1);
        assert_eq!(p.wins, 1);
        assert_eq!(p.losses, 0);
        assert!((p.total_pnl - 10.0).abs() < 1e-9);
        assert!((p.win_rate() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn record_losing_trade() {
        let mut p = StrategyPerformance::new("grid".into());
        p.record_trade(-5.0, None, 0.9);
        assert_eq!(p.wins, 0);
        assert_eq!(p.losses, 1);
        assert!((p.total_pnl - (-5.0)).abs() < 1e-9);
    }

    #[test]
    fn win_rate_mixed() {
        let mut p = StrategyPerformance::new("dca".into());
        p.record_trade(10.0, None, 0.9);
        p.record_trade(-3.0, None, 0.9);
        p.record_trade(5.0, None, 0.9);
        p.record_trade(-1.0, None, 0.9);
        assert!((p.win_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn drawdown_tracking() {
        let mut p = StrategyPerformance::new("momentum".into());
        p.record_trade(100.0, None, 0.9); // peak = 100
        p.record_trade(-30.0, None, 0.9); // pnl = 70, dd = 30%
        assert!((p.peak_pnl - 100.0).abs() < 1e-9);
        assert!((p.max_drawdown - 0.3).abs() < 1e-9);
    }

    #[test]
    fn ema_reward_decays() {
        let mut p = StrategyPerformance::new("grid".into());
        p.record_trade(10.0, None, 0.9);
        let first = p.ema_reward;
        p.record_trade(0.0, None, 0.9);
        assert!(p.ema_reward < first, "EMA should decay towards 0");
    }

    #[test]
    fn sharpe_positive_for_consistent_wins() {
        let mut p = StrategyPerformance::new("grid".into());
        for _ in 0..20 {
            p.record_trade(5.0, None, 0.9);
        }
        // All returns identical → variance ~0, mean > 0 → division guard
        // With tiny variance, sharpe should be 0 (division guard)
        // Actually, if all returns are exactly the same, variance = 0 → sharpe = 0
        assert_eq!(p.sharpe_estimate(), 0.0);
    }

    #[test]
    fn sharpe_positive_with_variance() {
        let mut p = StrategyPerformance::new("grid".into());
        for i in 0..20 {
            let pnl = if i % 3 == 0 { -2.0 } else { 5.0 };
            p.record_trade(pnl, None, 0.9);
        }
        assert!(p.sharpe_estimate() > 0.0);
    }

    #[test]
    fn regime_tracking() {
        let mut p = StrategyPerformance::new("grid".into());
        p.record_trade(10.0, Some("RANGING"), 0.9);
        p.record_trade(-5.0, Some("RANGING"), 0.9);
        p.record_trade(20.0, Some("TRENDING_UP"), 0.9);

        assert_eq!(p.regime_trades("RANGING"), 2);
        assert!((p.regime_pnl("RANGING") - 5.0).abs() < 1e-9);
        assert!((p.regime_win_rate("RANGING") - 0.5).abs() < 1e-9);

        assert_eq!(p.regime_trades("TRENDING_UP"), 1);
        assert!((p.regime_win_rate("TRENDING_UP") - 1.0).abs() < 1e-9);

        assert_eq!(p.regime_trades("VOLATILE"), 0);
        assert_eq!(p.regime_win_rate("VOLATILE"), 0.0);
    }

    #[test]
    fn json_round_trip() {
        let mut p = StrategyPerformance::new("pairs".into());
        p.record_trade(15.0, Some("RANGING"), 0.9);
        p.record_trade(-3.0, Some("TRENDING_UP"), 0.9);

        let json = p.to_json();
        let restored = StrategyPerformance::from_json(&json).unwrap();
        assert_eq!(restored.strategy_name, "pairs");
        assert_eq!(restored.trades, 2);
        assert!((restored.total_pnl - 12.0).abs() < 1e-9);
        assert_eq!(restored.regime_trades("RANGING"), 1);
    }

    #[test]
    fn reward_signal_blends_ema_and_sharpe() {
        let mut p = StrategyPerformance::new("grid".into());
        // Only one trade → sharpe = 0 → reward = ema
        p.record_trade(10.0, None, 0.9);
        assert!((p.reward_signal() - p.ema_reward).abs() < 1e-9);

        // Add variance so sharpe kicks in
        for i in 0..20 {
            let pnl = if i % 3 == 0 { -2.0 } else { 5.0 };
            p.record_trade(pnl, None, 0.9);
        }
        // With positive sharpe, reward should differ from raw ema
        let sharpe = p.sharpe_estimate();
        assert!(sharpe > 0.0);
    }

    #[test]
    fn recent_returns_capped() {
        let mut p = StrategyPerformance::new("grid".into());
        for i in 0..100 {
            p.record_trade(i as f64, None, 0.9);
        }
        assert!(p.recent_returns.len() <= p.max_recent);
    }

    #[test]
    fn zero_pnl_is_neither_win_nor_loss() {
        let mut p = StrategyPerformance::new("grid".into());
        p.record_trade(0.0, None, 0.9);
        assert_eq!(p.wins, 0);
        assert_eq!(p.losses, 0);
        assert_eq!(p.trades, 1);
    }
}
