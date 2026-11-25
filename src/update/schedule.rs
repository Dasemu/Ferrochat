//! When to check: 10-20 s after start, then every 6 h (with jitter), and
//! never in mock / screenshot / debug runs unless `FERRO_UPDATE_FORCE=1`.

use std::time::Duration;

/// Earliest first check after start.
pub const FIRST_MIN: Duration = Duration::from_secs(10);
/// Latest first check after start.
pub const FIRST_MAX: Duration = Duration::from_secs(20);
/// Time between checks.
pub const INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// Random spread around [`INTERVAL`] (either way).
pub const JITTER: Duration = Duration::from_secs(30 * 60);
/// Longest wait honoured from a rate-limit answer.
pub const MAX_BACKOFF: Duration = Duration::from_secs(24 * 3600);

/// `min + r mod (max - min)`.
fn between(min: Duration, max: Duration, r: u64) -> Duration {
    let span = max.saturating_sub(min).as_millis() as u64;
    if span == 0 {
        return min;
    }
    min + Duration::from_millis(r % span)
}

/// Delay of the first check (`r`: a random number).
pub fn first_delay(r: u64) -> Duration {
    between(FIRST_MIN, FIRST_MAX, r)
}

/// Delay until the next periodic check.
pub fn next_delay(r: u64) -> Duration {
    between(INTERVAL - JITTER, INTERVAL + JITTER, r)
}

/// Delay after a rate-limit answer: the server's wait if longer than the
/// normal interval, capped.
pub fn backoff(r: u64, retry_after: Option<Duration>) -> Duration {
    let normal = next_delay(r);
    retry_after
        .map(|w| w.min(MAX_BACKOFF).max(normal))
        .unwrap_or(normal)
}

/// Whether this run may check for updates at all.
pub fn checks_allowed(mock: bool, screenshot: bool, debug_build: bool, forced: bool) -> bool {
    forced || !(mock || screenshot || debug_build)
}

/// `FERRO_UPDATE_FORCE=1`.
pub fn forced() -> bool {
    std::env::var("FERRO_UPDATE_FORCE").is_ok_and(|v| v.trim() == "1")
}

/// A random number for the jitter (no need for anything stronger).
pub fn random() -> u64 {
    getrandom::u64().unwrap_or_else(|_| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_in_bounds() {
        for r in [0, 1, 9_999, 10_000, u64::MAX, random(), random()] {
            let f = first_delay(r);
            assert!(f >= FIRST_MIN && f < FIRST_MAX, "{f:?}");
            let n = next_delay(r);
            assert!(n >= INTERVAL - JITTER && n < INTERVAL + JITTER, "{n:?}");
        }
    }

    #[test]
    fn rate_limit_backoff() {
        assert!(backoff(0, None) >= INTERVAL - JITTER);
        assert!(backoff(0, Some(Duration::from_secs(60))) >= INTERVAL - JITTER);
        assert_eq!(
            backoff(0, Some(Duration::from_secs(10 * 3600))),
            Duration::from_secs(10 * 3600)
        );
        assert_eq!(backoff(0, Some(Duration::from_secs(1 << 30))), MAX_BACKOFF);
    }

    #[test]
    fn no_checks_in_mock_screenshot_or_debug_unless_forced() {
        assert!(checks_allowed(false, false, false, false));
        assert!(!checks_allowed(true, false, false, false));
        assert!(!checks_allowed(false, true, false, false));
        assert!(!checks_allowed(false, false, true, false));
        assert!(checks_allowed(true, true, true, true));
    }
}
