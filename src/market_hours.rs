#![allow(dead_code)]
//! Market session windows + holiday awareness. Pure logic, no network.
//!
//! NSE holidays are hardcoded for the current year; refresh annually from
//! <https://www.nseindia.com/resources/exchange-communication-holidays>.

use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Weekday};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// Continuous 24/7; always open.
    Crypto,
    /// NSE/BSE cash equity: 09:15 – 15:30 IST, Mon–Fri, minus NSE holidays.
    NseEquity,
    /// NSE F&O: equity hours for index/stock derivatives (currency has its own).
    NseFo,
    /// MCX commodities: 09:00 – 23:30 IST (winter), 09:00 – 23:55 (summer simplification skipped).
    Mcx,
    /// US equity regular session: 09:30 – 16:00 America/New_York, Mon–Fri, minus US holidays.
    UsEquity,
}

impl Session {
    pub fn is_open_at<T: TimeZone>(self, now: chrono::DateTime<T>) -> bool {
        match self {
            Session::Crypto => true,
            Session::NseEquity | Session::NseFo => {
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                if !is_weekday(t.weekday()) {
                    return false;
                }
                if is_nse_holiday(t.date_naive()) {
                    return false;
                }
                let tod = t.time();
                tod >= NaiveTime::from_hms_opt(9, 15, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(15, 30, 0).unwrap()
            }
            Session::Mcx => {
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                if !is_weekday(t.weekday()) {
                    return false;
                }
                // MCX observes its own holiday calendar; overlap with NSE's covers the common ones.
                if is_nse_holiday(t.date_naive()) {
                    return false;
                }
                let tod = t.time();
                tod >= NaiveTime::from_hms_opt(9, 0, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(23, 30, 0).unwrap()
            }
            Session::UsEquity => {
                let ny: Tz = chrono_tz::America::New_York;
                let t = now.with_timezone(&ny);
                if !is_weekday(t.weekday()) {
                    return false;
                }
                if is_us_equity_holiday(t.date_naive()) {
                    return false;
                }
                let tod = t.time();
                tod >= NaiveTime::from_hms_opt(9, 30, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(16, 0, 0).unwrap()
            }
        }
    }

    /// Returns `true` when an intraday position should be force-closed
    /// (15 minutes before session close).
    pub fn should_exit_intraday<T: TimeZone>(self, now: chrono::DateTime<T>) -> bool {
        match self {
            Session::Crypto => false,
            Session::NseEquity | Session::NseFo => {
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                let tod = t.time();
                // 15 min before 15:30 close => >= 15:15
                tod >= NaiveTime::from_hms_opt(15, 15, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(15, 30, 0).unwrap()
            }
            Session::Mcx => {
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                let tod = t.time();
                // 15 min before 23:30 close => >= 23:15
                tod >= NaiveTime::from_hms_opt(23, 15, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(23, 30, 0).unwrap()
            }
            Session::UsEquity => {
                let ny: Tz = chrono_tz::America::New_York;
                let t = now.with_timezone(&ny);
                let tod = t.time();
                // 15 min before 16:00 close => >= 15:45
                tod >= NaiveTime::from_hms_opt(15, 45, 0).unwrap()
                    && tod < NaiveTime::from_hms_opt(16, 0, 0).unwrap()
            }
        }
    }

    /// Minutes remaining until session close, or `None` if the session is
    /// closed or 24/7 (Crypto).
    pub fn minutes_until_close<T: TimeZone>(self, now: chrono::DateTime<T>) -> Option<i64> {
        match self {
            Session::Crypto => None,
            Session::NseEquity | Session::NseFo => {
                if !self.is_open_at(now.clone()) {
                    return None;
                }
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                let close = NaiveTime::from_hms_opt(15, 30, 0).unwrap();
                let diff = close - t.time();
                Some(diff.num_minutes())
            }
            Session::Mcx => {
                if !self.is_open_at(now.clone()) {
                    return None;
                }
                let ist: Tz = chrono_tz::Asia::Kolkata;
                let t = now.with_timezone(&ist);
                let close = NaiveTime::from_hms_opt(23, 30, 0).unwrap();
                let diff = close - t.time();
                Some(diff.num_minutes())
            }
            Session::UsEquity => {
                if !self.is_open_at(now.clone()) {
                    return None;
                }
                let ny: Tz = chrono_tz::America::New_York;
                let t = now.with_timezone(&ny);
                let close = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
                let diff = close - t.time();
                Some(diff.num_minutes())
            }
        }
    }
}

fn is_weekday(w: Weekday) -> bool {
    !matches!(w, Weekday::Sat | Weekday::Sun)
}

/// NSE trading holidays for 2026. Update annually.
/// Source: NSE circular list. Cross-check before relying on this in prod.
fn is_nse_holiday(d: NaiveDate) -> bool {
    const HOLIDAYS_2026: &[(i32, u32, u32)] = &[
        (2026, 1, 26),  // Republic Day
        (2026, 2, 17),  // Maha Shivratri (tentative)
        (2026, 3, 4),   // Holi (tentative)
        (2026, 3, 21),  // Eid-ul-Fitr (tentative)
        (2026, 4, 3),   // Good Friday
        (2026, 4, 14),  // Ambedkar Jayanti
        (2026, 5, 1),   // Maharashtra Day
        (2026, 5, 27),  // Eid-ul-Adha (tentative)
        (2026, 8, 15),  // Independence Day
        (2026, 8, 26),  // Ganesh Chaturthi (tentative)
        (2026, 10, 2),  // Gandhi Jayanti
        (2026, 10, 21), // Dussehra (tentative)
        (2026, 11, 9),  // Diwali / Laxmi Pujan (tentative)
        (2026, 11, 24), // Guru Nanak Jayanti (tentative)
        (2026, 12, 25), // Christmas
    ];
    HOLIDAYS_2026
        .iter()
        .any(|&(y, m, day)| d.year() == y && d.month() == m && d.day() == day)
}

/// NYSE trading holidays for 2026. Half-days not modelled (treated as full days).
fn is_us_equity_holiday(d: NaiveDate) -> bool {
    const HOLIDAYS_2026: &[(i32, u32, u32)] = &[
        (2026, 1, 1),   // New Year's Day
        (2026, 1, 19),  // MLK Day
        (2026, 2, 16),  // Washington's Birthday
        (2026, 4, 3),   // Good Friday
        (2026, 5, 25),  // Memorial Day
        (2026, 6, 19),  // Juneteenth
        (2026, 7, 3),   // Independence Day (observed)
        (2026, 9, 7),   // Labor Day
        (2026, 11, 26), // Thanksgiving
        (2026, 12, 25), // Christmas
    ];
    HOLIDAYS_2026
        .iter()
        .any(|&(y, m, day)| d.year() == y && d.month() == m && d.day() == day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Asia::Kolkata;

    #[test]
    fn nse_equity_open_at_1000_ist_weekday() {
        // 2026-04-23 is a Thursday and not in the 2026 holiday list.
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 10, 0, 0).unwrap();
        assert!(Session::NseEquity.is_open_at(t));
    }

    #[test]
    fn nse_equity_closed_at_0900_ist() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 9, 0, 0).unwrap();
        assert!(!Session::NseEquity.is_open_at(t));
    }

    #[test]
    fn nse_equity_closed_on_weekend() {
        // 2026-04-25 is a Saturday.
        let t = Kolkata.with_ymd_and_hms(2026, 4, 25, 10, 0, 0).unwrap();
        assert!(!Session::NseEquity.is_open_at(t));
    }

    #[test]
    fn nse_equity_closed_on_holiday() {
        let t = Kolkata.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap();
        assert!(!Session::NseEquity.is_open_at(t));
    }

    #[test]
    fn crypto_always_open() {
        let t = Kolkata.with_ymd_and_hms(2026, 1, 1, 3, 0, 0).unwrap();
        assert!(Session::Crypto.is_open_at(t));
    }

    // --- Time-based exit tests ---

    #[test]
    fn nse_should_exit_at_1520_ist() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 15, 20, 0).unwrap();
        assert!(Session::NseEquity.should_exit_intraday(t));
    }

    #[test]
    fn nse_should_not_exit_at_1400_ist() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 14, 0, 0).unwrap();
        assert!(!Session::NseEquity.should_exit_intraday(t));
    }

    #[test]
    fn crypto_never_forces_exit() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 15, 20, 0).unwrap();
        assert!(!Session::Crypto.should_exit_intraday(t));
        let t2 = Kolkata.with_ymd_and_hms(2026, 4, 23, 23, 59, 0).unwrap();
        assert!(!Session::Crypto.should_exit_intraday(t2));
    }

    #[test]
    fn us_should_exit_at_1550_et() {
        let ny: chrono_tz::Tz = chrono_tz::America::New_York;
        let t = ny.with_ymd_and_hms(2026, 4, 23, 15, 50, 0).unwrap();
        assert!(Session::UsEquity.should_exit_intraday(t));
    }

    #[test]
    fn minutes_until_close_nse_1400() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 14, 0, 0).unwrap();
        assert_eq!(Session::NseEquity.minutes_until_close(t), Some(90));
    }

    #[test]
    fn minutes_until_close_crypto_none() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 14, 0, 0).unwrap();
        assert_eq!(Session::Crypto.minutes_until_close(t), None);
    }

    #[test]
    fn mcx_should_exit_at_2320_ist() {
        let t = Kolkata.with_ymd_and_hms(2026, 4, 23, 23, 20, 0).unwrap();
        assert!(Session::Mcx.should_exit_intraday(t));
    }
}
