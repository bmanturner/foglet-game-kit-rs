//! System-clock-backed [`DateProvider`] for Murder Motel (SPEC_v2 §Task
//! 13a).
//!
//! The kit exposes [`foglet_game::DateProvider`] as a trait and ships a
//! [`foglet_game::FixedDateProvider`] for tests, but deliberately defers
//! shipping a wall-clock implementation (see `turns.rs` module docs:
//! "introduced in Task 6c when the first ledger-writing path actually
//! consumes it; deliberately not pre-built here to avoid landing dead
//! code"). Task 13a is the first ledger-writing path, so the example
//! brings its own.
//!
//! Implementation uses [`std::time::SystemTime`] plus Howard Hinnant's
//! `civil_from_days` algorithm to convert a UNIX-epoch day count to a
//! Gregorian `(year, month, day)` triple. We pull the algorithm in
//! verbatim rather than adding `chrono` or `time` to the workspace
//! crate budget — both would be a sizeable dependency for a single
//! call site, and the algorithm is both well-known and constant-time.
//!
//! Time zone: the date is computed in **UTC**. Murder Motel is the
//! kit's acceptance fixture; modelling local-midnight reset for an
//! arbitrary operator timezone would require either a system tzdb
//! lookup (`chrono-tz`) or a config-side IANA name. Both are out of
//! scope for v2's "prove the primitives" goal. SPEC_v2 §4.6 names
//! `local_midnight` as the configured reset; we honour the *daily*
//! shape of that contract while documenting the UTC simplification so
//! a future upgrade path is obvious.

use std::time::{SystemTime, UNIX_EPOCH};

use foglet_game::{DateProvider, LocalDate};

/// Wall-clock-backed [`DateProvider`] returning today's UTC date.
///
/// Stateless and zero-sized — every call reads `SystemTime::now()`
/// fresh. That matches the trait's "implementations must be cheap"
/// contract: the conversion is a handful of integer ops and the
/// allocation is one ten-byte `String` per call.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDateProvider;

impl DateProvider for SystemDateProvider {
    fn today(&self) -> LocalDate {
        // `duration_since(UNIX_EPOCH)` only fails when the system clock
        // is set before 1970-01-01 — a misconfiguration we will not
        // attempt to recover from. Falling back to the epoch keeps the
        // signature `LocalDate` (no `Result`) without panicking.
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let days = secs.div_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        LocalDate::parse(format!("{y:04}-{m:02}-{d:02}"))
            .expect("civil_from_days emits a valid YYYY-MM-DD shape")
    }
}

/// Howard Hinnant's `civil_from_days`: convert a count of days since
/// 1970-01-01 (proleptic Gregorian) to `(year, month, day)`.
///
/// Reference: <http://howardhinnant.github.io/date_algorithms.html>.
/// The algorithm is constant-time, branch-light, and correct across
/// the whole proleptic Gregorian range — overkill for a hotel game,
/// but it's the standard reference implementation for date math
/// without a dependency.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    // Shift the epoch onto 0000-03-01 so leap-year math is uniform
    // across centuries. 719_468 is the Hinnant constant.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    //! Spot-checks for [`SystemDateProvider`] and the underlying
    //! Hinnant conversion. We verify the date *shape* (10-byte
    //! `YYYY-MM-DD`) against the live system clock and pin a handful
    //! of well-known epoch days against the algorithm.

    use super::*;

    #[test]
    fn system_provider_returns_iso_shape() {
        let date = SystemDateProvider.today();
        let s = date.as_str();
        assert_eq!(s.len(), 10, "ISO date must be 10 bytes: {s}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        // Year sanity: this binary is being authored after 2024 and
        // before any wall-clock test machine plausibly hits 2100.
        let year: i32 = s[..4].parse().expect("year parses");
        assert!((2024..=2100).contains(&year), "year out of range: {year}");
    }

    #[test]
    fn civil_from_days_pins_unix_epoch() {
        // Day 0 is the UNIX epoch by definition.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_pins_known_dates() {
        // 2000-01-01 is 30 years + 7 leap-day adjustments after the
        // epoch; the Hinnant tables list the day count as 10_957.
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        // 2026-05-09 (today's date in this repo's CLAUDE config) is
        // 20_582 days after the epoch.
        assert_eq!(civil_from_days(20_582), (2026, 5, 9));
    }
}
