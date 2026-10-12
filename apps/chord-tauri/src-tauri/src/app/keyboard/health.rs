use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Keyboards disappear and reappear within milliseconds when Karabiner
/// recreates its virtual keyboard or the machine wakes. Input ownership only
/// drops after the Caps Lock source has been gone for this long.
pub const CAPS_DEVICE_GRACE: Duration = Duration::from_secs(2);

/// Live status of the two input listeners, written by the listener threads
/// and read by the supervisor and the input-ownership lease.
#[derive(Default)]
pub struct InputHealth {
    tap_running: AtomicBool,
    tap_enabled: AtomicBool,
    caps_running: AtomicBool,
    caps_open_devices: AtomicUsize,
    caps_devices_lost_at: Mutex<Option<Instant>>,
}

impl InputHealth {
    pub fn tap_running(&self) -> bool {
        self.tap_running.load(Ordering::Acquire)
    }

    pub fn tap_enabled(&self) -> bool {
        self.tap_enabled.load(Ordering::Acquire)
    }

    pub fn caps_running(&self) -> bool {
        self.caps_running.load(Ordering::Acquire)
    }

    pub fn caps_open_devices(&self) -> usize {
        self.caps_open_devices.load(Ordering::Acquire)
    }

    /// Whether Caps Lock presses can currently be observed, allowing for a
    /// short device re-enumeration gap.
    pub fn caps_ready(&self, now: Instant) -> bool {
        if !self.caps_running() {
            return false;
        }
        if self.caps_open_devices() > 0 {
            return true;
        }
        self.caps_devices_lost_at()
            .is_some_and(|lost_at| now.duration_since(lost_at) < CAPS_DEVICE_GRACE)
    }

    /// When the last readable keyboard disappeared, if none is open.
    pub fn caps_devices_lost_at(&self) -> Option<Instant> {
        lock_ignoring_poison(&self.caps_devices_lost_at).and_then(|guard| *guard)
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

    pub(super) fn set_caps_running(&self, running: bool, now: Instant) {
        self.caps_running.store(running, Ordering::Release);
        self.caps_open_devices.store(0, Ordering::Release);
        if let Some(mut lost_at) = lock_ignoring_poison(&self.caps_devices_lost_at) {
            // A fresh listener has not lost anything yet; a stopped one has no source.
            *lost_at = running.then_some(now);
        }
    }

    pub(super) fn set_caps_open_devices(&self, count: usize, now: Instant) {
        let previous = self.caps_open_devices.swap(count, Ordering::AcqRel);
        if let Some(mut lost_at) = lock_ignoring_poison(&self.caps_devices_lost_at) {
            if count > 0 {
                *lost_at = None;
            } else if previous > 0 || lost_at.is_none() {
                *lost_at = Some(now);
            }
        }
    }
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> Option<std::sync::MutexGuard<'_, T>> {
    Some(mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
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
    fn caps_readiness_survives_a_short_device_gap_only() {
        let health = InputHealth::default();
        let start = Instant::now();
        assert!(!health.caps_ready(start));

        health.set_caps_running(true, start);
        // Running without any keyboard yet: not ready, but within the grace window
        // a keyboard is still expected.
        assert!(!health.caps_ready(start + CAPS_DEVICE_GRACE));

        health.set_caps_open_devices(1, start);
        assert!(health.caps_ready(start + Duration::from_secs(60)));

        let lost = start + Duration::from_secs(60);
        health.set_caps_open_devices(0, lost);
        assert!(health.caps_ready(lost + CAPS_DEVICE_GRACE / 2));
        assert!(!health.caps_ready(lost + CAPS_DEVICE_GRACE));

        health.set_caps_open_devices(2, lost + Duration::from_secs(5));
        assert!(health.caps_ready(lost + Duration::from_secs(6)));

        health.set_caps_running(false, lost + Duration::from_secs(7));
        assert!(!health.caps_ready(lost + Duration::from_secs(7)));
        assert_eq!(health.caps_open_devices(), 0);
    }

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
