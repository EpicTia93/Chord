use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Live status of the keyboard event tap, written by the tap thread and read
/// by the supervisor and the input-ownership lease.
#[derive(Default)]
pub struct InputHealth {
    tap_running: AtomicBool,
    tap_enabled: AtomicBool,
}

impl InputHealth {
    pub fn tap_running(&self) -> bool {
        self.tap_running.load(Ordering::Acquire)
    }

    pub fn tap_enabled(&self) -> bool {
        self.tap_enabled.load(Ordering::Acquire)
    }

    pub(super) fn set_tap_running(&self, running: bool) {
        self.tap_running.store(running, Ordering::Release);
        if !running {
            self.tap_enabled.store(false, Ordering::Release);
        }
    }

    pub(super) fn set_tap_enabled(&self, enabled: bool) {
        self.tap_enabled.store(enabled, Ordering::Release);
    }
}

/// Exponential retry schedule for restarting a listener after it failed.
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
    retry_at: Option<Instant>,
}

impl Backoff {
    const DELAYS: [Duration; 6] = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(16),
        Duration::from_secs(30),
    ];

    pub fn ready(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|retry_at| now >= retry_at)
    }

    pub fn failed(&mut self, now: Instant) -> Duration {
        let delay = Self::DELAYS[(self.failures as usize).min(Self::DELAYS.len() - 1)];
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + delay);
        delay
    }

    pub fn succeeded(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_resets() {
        let mut backoff = Backoff::default();
        let now = Instant::now();
        assert!(backoff.ready(now));
        assert_eq!(backoff.failed(now), Duration::from_secs(1));
        assert!(!backoff.ready(now));
        assert!(backoff.ready(now + Duration::from_secs(1)));
        assert_eq!(backoff.failed(now), Duration::from_secs(2));
        for _ in 0..10 {
            backoff.failed(now);
        }
        assert_eq!(backoff.failed(now), Duration::from_secs(30));
        backoff.succeeded();
        assert!(backoff.ready(now));
        assert_eq!(backoff.failed(now), Duration::from_secs(1));
    }
}
