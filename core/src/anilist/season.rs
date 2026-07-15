//! AniList season math. Pure, so the current/next season computation is
//! unit-tested rather than eyeballed against a calendar.
//!
//! AniList buckets anime into four seasons by month:
//!   WINTER = Jan–Mar, SPRING = Apr–Jun, SUMMER = Jul–Sep, FALL = Oct–Dec.

/// An AniList `MediaSeason` enum value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Winter,
    Spring,
    Summer,
    Fall,
}

impl Season {
    pub fn as_str(&self) -> &'static str {
        match self {
            Season::Winter => "WINTER",
            Season::Spring => "SPRING",
            Season::Summer => "SUMMER",
            Season::Fall => "FALL",
        }
    }

    /// The season for a 1-based calendar month (1..=12). Out-of-range months
    /// clamp into a valid season rather than panic.
    pub fn from_month(month: u32) -> Season {
        match month {
            1..=3 => Season::Winter,
            4..=6 => Season::Spring,
            7..=9 => Season::Summer,
            _ => Season::Fall, // 10..=12 and any out-of-range value
        }
    }

    /// The season after this one, plus whether the year rolled over (FALL→WINTER
    /// advances the year).
    pub fn next(&self) -> (Season, bool) {
        match self {
            Season::Winter => (Season::Spring, false),
            Season::Spring => (Season::Summer, false),
            Season::Summer => (Season::Fall, false),
            Season::Fall => (Season::Winter, true),
        }
    }
}

/// The current and next (season, year) pair for a given calendar year + month.
pub fn current_and_next(year: i64, month: u32) -> ((Season, i64), (Season, i64)) {
    let cur = Season::from_month(month);
    let (nxt, rolled) = cur.next();
    let next_year = if rolled { year + 1 } else { year };
    ((cur, year), (nxt, next_year))
}

/// Convert a unix timestamp (seconds) to (year, 1-based month) in UTC, using a
/// self-contained civil-calendar algorithm so we need no chrono dependency.
/// Based on Howard Hinnant's days-from-civil inverse.
pub fn ymd_from_unix(secs: i64) -> (i64, u32, u32) {
    let days = secs.div_euclid(86_400);
    // Shift epoch to 0000-03-01 to make leap-year handling uniform.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// The current and next (season, year) pair for a unix timestamp (UTC).
pub fn current_and_next_from_unix(secs: i64) -> ((Season, i64), (Season, i64)) {
    let (year, month, _) = ymd_from_unix(secs);
    current_and_next(year, month)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_from_month_buckets() {
        assert_eq!(Season::from_month(1), Season::Winter);
        assert_eq!(Season::from_month(3), Season::Winter);
        assert_eq!(Season::from_month(4), Season::Spring);
        assert_eq!(Season::from_month(7), Season::Summer);
        assert_eq!(Season::from_month(9), Season::Summer);
        assert_eq!(Season::from_month(10), Season::Fall);
        assert_eq!(Season::from_month(12), Season::Fall);
        // Out-of-range clamps rather than panicking.
        assert_eq!(Season::from_month(0), Season::Fall);
        assert_eq!(Season::from_month(99), Season::Fall);
    }

    #[test]
    fn current_and_next_rolls_year_at_fall() {
        // July 2026 → SUMMER 2026, next FALL 2026 (no roll).
        let ((cs, cy), (ns, ny)) = current_and_next(2026, 7);
        assert_eq!((cs, cy), (Season::Summer, 2026));
        assert_eq!((ns, ny), (Season::Fall, 2026));

        // December 2026 → FALL 2026, next WINTER 2027 (year rolls).
        let ((cs, cy), (ns, ny)) = current_and_next(2026, 12);
        assert_eq!((cs, cy), (Season::Fall, 2026));
        assert_eq!((ns, ny), (Season::Winter, 2027));
    }

    #[test]
    fn ymd_from_unix_known_dates() {
        // 2026-07-15 00:00:00 UTC = 1_784_073_600
        assert_eq!(ymd_from_unix(1_784_073_600), (2026, 7, 15));
        // Unix epoch.
        assert_eq!(ymd_from_unix(0), (1970, 1, 1));
        // 2000-02-29 (leap day) 12:00 UTC = 951_825_600
        assert_eq!(ymd_from_unix(951_825_600), (2000, 2, 29));
    }

    #[test]
    fn current_and_next_from_unix_matches() {
        // 2026-07-15 → SUMMER 2026 / FALL 2026.
        let ((cs, cy), (ns, ny)) = current_and_next_from_unix(1_784_073_600);
        assert_eq!((cs, cy), (Season::Summer, 2026));
        assert_eq!((ns, ny), (Season::Fall, 2026));
    }
}
