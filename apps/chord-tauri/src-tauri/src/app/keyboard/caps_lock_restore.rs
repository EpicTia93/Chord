//! Keeps Caps+Space from changing the Caps Lock toggle.
//!
//! macOS toggles Caps Lock on key-down (after its hold delay on Apple
//! keyboards), so by the time Space is pressed the toggle has usually flipped.
//! A plain Caps press must keep toggling as usual; a press that activated
//! Chord mode must leave the toggle exactly as it was before the press.
//!
//! The "before" state is never read at press time: the OS toggle can race
//! the HID callback. It is the last state observed while the key was up.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct CapsLockRestore {
    /// OS toggle state last observed while Caps Lock was not held.
    last_known: AtomicBool,
    /// Snapshot of `last_known` taken when the key went down.
    before_press: AtomicBool,
    /// Chord mode was activated during the current hold.
    activated: AtomicBool,
}

impl CapsLockRestore {
    /// Records the OS toggle state. Only meaningful while the key is up.
    pub fn observe(&self, enabled: bool) {
        self.last_known.store(enabled, Ordering::SeqCst);
    }

    pub fn last_known(&self) -> bool {
        self.last_known.load(Ordering::SeqCst)
    }

    pub fn pressed(&self) {
        self.before_press
            .store(self.last_known.load(Ordering::SeqCst), Ordering::SeqCst);
        self.activated.store(false, Ordering::SeqCst);
    }

    /// Chord mode was entered while Caps is held. Returns the state to put
    /// the OS toggle back to right away; the release restores it again in
    /// case the delayed OS toggle lands after this point.
    pub fn chord_activated(&self) -> bool {
        self.activated.store(true, Ordering::SeqCst);
        self.before_press.load(Ordering::SeqCst)
    }

    /// Returns the state to restore if this hold activated Chord mode.
    pub fn released(&self) -> Option<bool> {
        self.activated
            .swap(false, Ordering::SeqCst)
            .then(|| self.before_press.load(Ordering::SeqCst))
    }

    pub fn reset(&self) {
        self.activated.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_press_leaves_the_toggle_alone() {
        let restore = CapsLockRestore::default();
        restore.observe(false);
        restore.pressed();
        assert_eq!(restore.released(), None);
        // The OS toggled it on; that becomes the new baseline.
        restore.observe(true);
        restore.pressed();
        assert_eq!(restore.released(), None);
    }

    #[test]
    fn chord_activation_restores_the_state_before_the_press() {
        for before in [false, true] {
            let restore = CapsLockRestore::default();
            restore.observe(before);
            restore.pressed();
            assert_eq!(restore.chord_activated(), before);
            assert_eq!(restore.released(), Some(before));
            // Restoring is a one-shot per hold.
            assert_eq!(restore.released(), None);
        }
    }

    #[test]
    fn observations_during_a_hold_do_not_change_the_snapshot() {
        let restore = CapsLockRestore::default();
        restore.observe(false);
        restore.pressed();
        restore.observe(true);
        assert_eq!(restore.chord_activated(), false);
        assert_eq!(restore.released(), Some(false));
    }

    #[test]
    fn reset_forgets_an_activation() {
        let restore = CapsLockRestore::default();
        restore.pressed();
        restore.chord_activated();
        restore.reset();
        assert_eq!(restore.released(), None);
    }
}
