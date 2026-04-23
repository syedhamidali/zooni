#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderKind {
    Market,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_serialises_uppercase() {
        assert_eq!(serde_json::to_string(&Side::Buy).unwrap(), "\"BUY\"");
        assert_eq!(serde_json::to_string(&Side::Sell).unwrap(), "\"SELL\"");
    }

    #[test]
    fn side_deserialises_uppercase() {
        let buy: Side = serde_json::from_str("\"BUY\"").unwrap();
        assert_eq!(buy, Side::Buy);
        let sell: Side = serde_json::from_str("\"SELL\"").unwrap();
        assert_eq!(sell, Side::Sell);
    }

    #[test]
    fn order_kind_round_trip() {
        let kinds = [OrderKind::Market, OrderKind::Limit];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let back: OrderKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*kind, back);
        }
    }

    #[test]
    fn time_in_force_round_trip() {
        let tifs = [TimeInForce::Gtc, TimeInForce::Ioc, TimeInForce::Fok];
        for tif in &tifs {
            let json = serde_json::to_string(tif).unwrap();
            let back: TimeInForce = serde_json::from_str(&json).unwrap();
            assert_eq!(*tif, back);
        }
    }
}
