//! Tax-aware profitability engine for the Indian market.
//!
//! Indian crypto: 30% flat tax on gains (no loss offsetting) + 1% TDS on every sale.
//! Indian equity: 15% STCG (held < 1 year), 10% LTCG (held > 1 year, above 1L exemption).
//! International (e.g. Bybit): no automatic tax deduction — user handles it themselves.
//!
//! This module is pure — no I/O, no side effects. Strategies call `TaxEngine` to
//! check whether a trade is actually profitable after all deductions.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxJurisdiction {
    India,
    #[default]
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetClass {
    CryptoIndia,
    EquityIndia,
    CryptoInternational,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaxConfig {
    #[serde(default)]
    pub jurisdiction: TaxJurisdiction,
    #[serde(default = "default_crypto_tax_rate")]
    pub crypto_tax_rate: f64,
    #[serde(default = "default_crypto_tds_rate")]
    pub crypto_tds_rate: f64,
    #[serde(default = "default_equity_stcg_rate")]
    pub equity_stcg_rate: f64,
    #[serde(default = "default_equity_ltcg_rate")]
    pub equity_ltcg_rate: f64,
    #[serde(default = "default_trading_fee_rate")]
    pub trading_fee_rate: f64,
}

fn default_crypto_tax_rate() -> f64 {
    0.30
}
fn default_crypto_tds_rate() -> f64 {
    0.01
}
fn default_equity_stcg_rate() -> f64 {
    0.15
}
fn default_equity_ltcg_rate() -> f64 {
    0.10
}
fn default_trading_fee_rate() -> f64 {
    0.001
}

impl Default for TaxConfig {
    fn default() -> Self {
        Self {
            jurisdiction: TaxJurisdiction::default(),
            crypto_tax_rate: default_crypto_tax_rate(),
            crypto_tds_rate: default_crypto_tds_rate(),
            equity_stcg_rate: default_equity_stcg_rate(),
            equity_ltcg_rate: default_equity_ltcg_rate(),
            trading_fee_rate: default_trading_fee_rate(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaxAssessment {
    pub gross_profit: f64,
    pub trading_fees: f64,
    pub tds_amount: f64,
    pub tax_amount: f64,
    pub net_profit: f64,
    pub effective_rate: f64,
    pub is_profitable: bool,
}

pub struct TaxEngine {
    config: TaxConfig,
}

impl TaxEngine {
    pub fn new(config: TaxConfig) -> Self {
        Self { config }
    }

    pub fn is_trade_profitable(
        &self,
        entry_price: f64,
        exit_price: f64,
        qty: f64,
        asset_class: AssetClass,
    ) -> TaxAssessment {
        let entry_value = entry_price * qty;
        let exit_value = exit_price * qty;
        let gross_profit = exit_value - entry_value;

        // Trading fees apply on both entry and exit legs.
        let trading_fees = (entry_value + exit_value) * self.config.trading_fee_rate;

        // TDS and tax depend on asset class.
        let (tds_amount, tax_amount) = match asset_class {
            AssetClass::CryptoIndia => {
                // 1% TDS on the sale (exit) value.
                let tds = exit_value * self.config.crypto_tds_rate;
                // 30% tax on gains only (not on losses — but you still pay TDS).
                let tax = if gross_profit > 0.0 {
                    gross_profit * self.config.crypto_tax_rate
                } else {
                    0.0
                };
                (tds, tax)
            }
            AssetClass::EquityIndia => {
                // No TDS on equity trades in India.
                let tax = if gross_profit > 0.0 {
                    gross_profit * self.config.equity_stcg_rate
                } else {
                    0.0
                };
                (0.0, tax)
            }
            AssetClass::CryptoInternational => (0.0, 0.0),
        };

        let net_profit = gross_profit - trading_fees - tds_amount - tax_amount;

        let effective_rate = if gross_profit.abs() > f64::EPSILON {
            (trading_fees + tds_amount + tax_amount) / gross_profit.abs()
        } else {
            0.0
        };

        TaxAssessment {
            gross_profit,
            trading_fees,
            tds_amount,
            tax_amount,
            net_profit,
            effective_rate,
            is_profitable: net_profit > 0.0,
        }
    }

    /// Minimum gross profit percentage needed to break even after tax + fees + TDS.
    ///
    /// For Indian crypto: you need enough profit so that after 30% tax, 1% TDS on
    /// the sale, and ~0.2% round-trip fees, you still net >= 0.
    ///
    /// Derivation (crypto):
    ///   Let P = entry price, r = profit ratio, so exit = P*(1+r), qty = 1.
    ///   gross_profit = P*r
    ///   fees = (P + P*(1+r)) * fee_rate = P*(2+r)*fee_rate
    ///   tds  = P*(1+r)*tds_rate
    ///   tax  = P*r*tax_rate    (only when r > 0)
    ///   break-even: P*r - P*(2+r)*fee_rate - P*(1+r)*tds_rate - P*r*tax_rate = 0
    ///   => r - (2+r)*fee - (1+r)*tds - r*tax = 0
    ///   => r*(1 - fee - tds - tax) = 2*fee + tds
    ///   => r = (2*fee + tds) / (1 - fee - tds - tax)
    pub fn min_profit_pct(&self, asset_class: AssetClass) -> f64 {
        let fee = self.config.trading_fee_rate;
        match asset_class {
            AssetClass::CryptoIndia => {
                let tds = self.config.crypto_tds_rate;
                let tax = self.config.crypto_tax_rate;
                let denom = 1.0 - fee - tds - tax;
                if denom <= 0.0 {
                    return f64::INFINITY;
                }
                (2.0 * fee + tds) / denom
            }
            AssetClass::EquityIndia => {
                let tax = self.config.equity_stcg_rate;
                let denom = 1.0 - fee - tax;
                if denom <= 0.0 {
                    return f64::INFINITY;
                }
                (2.0 * fee) / denom
            }
            AssetClass::CryptoInternational => {
                let denom = 1.0 - fee;
                if denom <= 0.0 {
                    return f64::INFINITY;
                }
                (2.0 * fee) / denom
            }
        }
    }

    /// Total effective deduction rate on a profitable trade (tax + TDS + fees as
    /// fraction of gross profit). Assumes a small profit so fees ~ 2*fee_rate*entry.
    pub fn effective_tax_rate(&self, asset_class: AssetClass) -> f64 {
        let fee = self.config.trading_fee_rate;
        match asset_class {
            AssetClass::CryptoIndia => {
                self.config.crypto_tax_rate + self.config.crypto_tds_rate + 2.0 * fee
            }
            AssetClass::EquityIndia => self.config.equity_stcg_rate + 2.0 * fee,
            AssetClass::CryptoInternational => 2.0 * fee,
        }
    }

    /// Quick net-after-tax calculation on a known gross profit amount.
    /// For losses, no tax is applied but TDS is still deducted for Indian crypto.
    pub fn net_after_tax(&self, gross_profit: f64, asset_class: AssetClass) -> f64 {
        match asset_class {
            AssetClass::CryptoIndia => {
                if gross_profit > 0.0 {
                    gross_profit
                        * (1.0
                            - self.config.crypto_tax_rate
                            - self.config.crypto_tds_rate
                            - 2.0 * self.config.trading_fee_rate)
                } else {
                    // Loss: no tax, but TDS + fees still bite.
                    gross_profit
                        - gross_profit.abs()
                            * (self.config.crypto_tds_rate + 2.0 * self.config.trading_fee_rate)
                }
            }
            AssetClass::EquityIndia => {
                if gross_profit > 0.0 {
                    gross_profit
                        * (1.0 - self.config.equity_stcg_rate - 2.0 * self.config.trading_fee_rate)
                } else {
                    gross_profit - gross_profit.abs() * 2.0 * self.config.trading_fee_rate
                }
            }
            AssetClass::CryptoInternational => {
                if gross_profit > 0.0 {
                    gross_profit * (1.0 - 2.0 * self.config.trading_fee_rate)
                } else {
                    gross_profit - gross_profit.abs() * 2.0 * self.config.trading_fee_rate
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn india_config() -> TaxConfig {
        TaxConfig {
            jurisdiction: TaxJurisdiction::India,
            ..TaxConfig::default()
        }
    }

    fn engine() -> TaxEngine {
        TaxEngine::new(india_config())
    }

    #[test]
    fn crypto_india_small_trade_unprofitable() {
        let e = engine();
        // 0.5% gross gain — should be eaten by 30% tax + 1% TDS + fees.
        let a = e.is_trade_profitable(100.0, 100.5, 10.0, AssetClass::CryptoIndia);
        assert!(
            !a.is_profitable,
            "small crypto trade should be unprofitable after Indian tax"
        );
        assert!(a.tax_amount > 0.0);
        assert!(a.tds_amount > 0.0);
    }

    #[test]
    fn crypto_india_large_trade_profitable() {
        let e = engine();
        // 5% gross gain — should survive tax.
        let a = e.is_trade_profitable(100.0, 105.0, 10.0, AssetClass::CryptoIndia);
        assert!(
            a.is_profitable,
            "5% gain should be profitable even after Indian crypto tax"
        );
    }

    #[test]
    fn crypto_india_break_even_threshold() {
        let e = engine();
        let threshold = e.min_profit_pct(AssetClass::CryptoIndia);
        // threshold should be around 1.76% for default rates.
        assert!(threshold > 0.01, "break-even should be > 1%");
        assert!(threshold < 0.05, "break-even should be < 5%");

        // A trade at exactly the threshold should be ~break-even.
        let entry = 100.0;
        let exit = entry * (1.0 + threshold);
        let a = e.is_trade_profitable(entry, exit, 1.0, AssetClass::CryptoIndia);
        assert!(
            a.net_profit.abs() < 0.01,
            "trade at threshold should be ~break-even, got net={}",
            a.net_profit
        );
    }

    #[test]
    fn equity_india_stcg() {
        let e = engine();
        let a = e.is_trade_profitable(100.0, 110.0, 100.0, AssetClass::EquityIndia);
        // gross = 1000, tax = 150 (15%), fees ~ 21 (0.1% of 21000), no TDS.
        assert!(a.is_profitable);
        assert!(
            (a.tax_amount - 150.0).abs() < 0.01,
            "STCG should be 15% of 1000"
        );
        assert_eq!(a.tds_amount, 0.0, "no TDS on equity");
    }

    #[test]
    fn international_crypto_no_tax() {
        let e = engine();
        let a = e.is_trade_profitable(100.0, 105.0, 10.0, AssetClass::CryptoInternational);
        assert_eq!(a.tax_amount, 0.0);
        assert_eq!(a.tds_amount, 0.0);
        assert!(a.is_profitable);
        // Only fees deducted.
        let expected_fees = (1000.0 + 1050.0) * 0.001;
        assert!((a.trading_fees - expected_fees).abs() < 0.001);
    }

    #[test]
    fn zero_profit_trade() {
        let e = engine();
        let a = e.is_trade_profitable(100.0, 100.0, 10.0, AssetClass::CryptoIndia);
        assert_eq!(a.gross_profit, 0.0);
        assert_eq!(a.tax_amount, 0.0);
        // TDS still applies on sale value, fees still apply.
        assert!(a.tds_amount > 0.0);
        assert!(a.trading_fees > 0.0);
        assert!(
            !a.is_profitable,
            "zero gross should be net negative after fees + TDS"
        );
    }

    #[test]
    fn loss_trade_no_tax_but_tds_applies() {
        let e = engine();
        let a = e.is_trade_profitable(100.0, 95.0, 10.0, AssetClass::CryptoIndia);
        assert!(a.gross_profit < 0.0);
        assert_eq!(a.tax_amount, 0.0, "no tax on losses");
        assert!(a.tds_amount > 0.0, "TDS still applies on sale");
        assert!(!a.is_profitable);
        // Net loss should be worse than gross loss.
        assert!(a.net_profit < a.gross_profit);
    }

    #[test]
    fn min_profit_pct_each_asset_class() {
        let e = engine();
        let crypto_in = e.min_profit_pct(AssetClass::CryptoIndia);
        let equity_in = e.min_profit_pct(AssetClass::EquityIndia);
        let crypto_intl = e.min_profit_pct(AssetClass::CryptoInternational);

        // Indian crypto is the most punishing.
        assert!(
            crypto_in > equity_in,
            "crypto India threshold > equity India"
        );
        assert!(
            equity_in > crypto_intl,
            "equity India threshold > international"
        );
        // International should be tiny — just fees.
        assert!(
            crypto_intl < 0.005,
            "international break-even should be < 0.5%"
        );
    }

    #[test]
    fn profitable_pretax_unprofitable_posttax() {
        let e = engine();
        // 1% gross gain on Indian crypto — profitable in isolation, but not after 30% tax + TDS.
        let a = e.is_trade_profitable(100.0, 101.0, 10.0, AssetClass::CryptoIndia);
        assert!(a.gross_profit > 0.0, "gross should be positive");
        assert!(
            !a.is_profitable,
            "should be unprofitable after tax+TDS+fees"
        );
    }

    #[test]
    fn effective_tax_rate_values() {
        let e = engine();
        let crypto_rate = e.effective_tax_rate(AssetClass::CryptoIndia);
        // 30% tax + 1% TDS + 0.2% fees = 31.2%
        assert!((crypto_rate - 0.312).abs() < 0.001);

        let equity_rate = e.effective_tax_rate(AssetClass::EquityIndia);
        // 15% STCG + 0.2% fees = 15.2%
        assert!((equity_rate - 0.152).abs() < 0.001);

        let intl_rate = e.effective_tax_rate(AssetClass::CryptoInternational);
        // just 0.2% fees
        assert!((intl_rate - 0.002).abs() < 0.001);
    }

    #[test]
    fn net_after_tax_positive_profit() {
        let e = engine();
        let net = e.net_after_tax(1000.0, AssetClass::CryptoIndia);
        // 1000 * (1 - 0.30 - 0.01 - 0.002) = 1000 * 0.688 = 688
        assert!((net - 688.0).abs() < 0.01);
    }

    #[test]
    fn break_even_price_equity() {
        let e = engine();
        let threshold = e.min_profit_pct(AssetClass::EquityIndia);
        let entry = 500.0;
        let exit = entry * (1.0 + threshold);
        let a = e.is_trade_profitable(entry, exit, 100.0, AssetClass::EquityIndia);
        assert!(
            a.net_profit.abs() < 0.10,
            "equity trade at threshold should be ~break-even, got net={}",
            a.net_profit
        );
    }

    #[test]
    fn default_config_is_sane() {
        let cfg = TaxConfig::default();
        assert_eq!(cfg.jurisdiction, TaxJurisdiction::None);
        assert_eq!(cfg.crypto_tax_rate, 0.30);
        assert_eq!(cfg.crypto_tds_rate, 0.01);
        assert_eq!(cfg.equity_stcg_rate, 0.15);
        assert_eq!(cfg.equity_ltcg_rate, 0.10);
        assert_eq!(cfg.trading_fee_rate, 0.001);
    }

    #[test]
    fn serde_deserialize_jurisdiction() {
        let json = r#""india""#;
        let j: TaxJurisdiction = serde_json::from_str(json).unwrap();
        assert_eq!(j, TaxJurisdiction::India);

        let json = r#""none""#;
        let j: TaxJurisdiction = serde_json::from_str(json).unwrap();
        assert_eq!(j, TaxJurisdiction::None);
    }
}
