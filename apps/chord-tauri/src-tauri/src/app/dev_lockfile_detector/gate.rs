use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

/// The generation is plain data, so a panic while it was held (in a handler
/// or a handoff reset) must not disable input for the rest of the process.
#[derive(Default)]
pub(super) struct InputGate(RwLock<Option<u64>>);

impl InputGate {
    /// Event taps must never wait for a handoff or slow controller operation.
    pub(super) fn capture<T>(&self, f: impl FnOnce(u64) -> T) -> Option<T> {
        let generation = match self.0.try_read() {
            Ok(generation) => generation,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        generation.map(f)
    }

    /// Like [`Self::capture`], but waits for an in-progress handoff instead of
    /// dropping the event. Only for callbacks that are not under a deadline.
    pub(super) fn capture_blocking<T>(&self, f: impl FnOnce(u64) -> T) -> Option<T> {
        let generation = self.read();
        generation.map(f)
    }

    pub(super) fn process<T>(&self, expected: u64, f: impl FnOnce() -> T) -> Option<T> {
        let generation = self.read();
        (*generation == Some(expected)).then(f)
    }

    pub(super) fn transition(&self, next: Option<u64>, reset: impl FnOnce()) {
        // Drain controller work before the caller releases the OS owner lock.
        let mut generation = self.write();
        *generation = None;
        reset();
        *generation = next;
    }

    fn read(&self) -> RwLockReadGuard<'_, Option<u64>> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Option<u64>> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn queued_events_cannot_cross_a_handoff_or_reactivation() {
        let gate = InputGate::default();
        assert_eq!(gate.capture(|generation| generation), None);
        gate.transition(Some(1), || {});
        let queued = gate.capture(|generation| generation).unwrap();
        assert_eq!(gate.process(queued, || 42), Some(42));
        gate.transition(None, || {});
        assert_eq!(gate.process(queued, || 42), None);
        gate.transition(Some(2), || {});
        assert_eq!(gate.process(queued, || 42), None);
        assert_eq!(gate.process(2, || 42), Some(42));
    }

    #[test]
    fn handoff_waits_for_in_flight_work_and_callbacks_do_not_block_during_reset() {
        let gate = InputGate::default();
        gate.transition(Some(1), || {});
        std::thread::scope(|scope| {
            let (started_tx, started_rx) = mpsc::channel();
            let (finish_tx, finish_rx) = mpsc::channel();
            let gate_ref = &gate;
            scope.spawn(move || {
                gate_ref.process(1, || {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                })
            });
            started_rx.recv().unwrap();
            let (reset_tx, reset_rx) = mpsc::channel();
            let (transition_tx, transition_rx) = mpsc::channel();
            scope.spawn(move || {
                transition_tx.send(()).unwrap();
                gate_ref.transition(None, || {
                    // try_read must return immediately while the writer owns the gate.
                    assert!(gate_ref.capture(|_| ()).is_none());
                    reset_tx.send(()).unwrap();
                });
            });
            transition_rx.recv().unwrap();
            assert!(reset_rx.recv_timeout(Duration::from_millis(30)).is_err());
            finish_tx.send(()).unwrap();
            reset_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        assert!(gate.capture(|_| ()).is_none());
    }

    #[test]
    fn blocking_capture_sees_the_generation_after_a_handoff_completes() {
        let gate = InputGate::default();
        gate.transition(Some(1), || {});
        std::thread::scope(|scope| {
            let (reset_started_tx, reset_started_rx) = mpsc::channel();
            let (finish_reset_tx, finish_reset_rx) = mpsc::channel();
            let gate_ref = &gate;
            scope.spawn(move || {
                gate_ref.transition(Some(2), || {
                    reset_started_tx.send(()).unwrap();
                    finish_reset_rx.recv().unwrap();
                });
            });
            reset_started_rx.recv().unwrap();
            let (captured_tx, captured_rx) = mpsc::channel();
            scope.spawn(move || {
                captured_tx
                    .send(gate_ref.capture_blocking(|generation| generation))
                    .unwrap();
            });
            assert!(captured_rx.recv_timeout(Duration::from_millis(30)).is_err());
            finish_reset_tx.send(()).unwrap();
            assert_eq!(
                captured_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                Some(2)
            );
        });
    }

    #[test]
    fn a_panic_during_a_handoff_does_not_poison_input_forever() {
        let gate = InputGate::default();
        gate.transition(Some(1), || {});
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            gate.transition(Some(2), || panic!("reset failed"));
        }));
        assert!(result.is_err());
        // The panicking reset left the gate closed; a later handoff reopens it.
        assert_eq!(gate.capture(|generation| generation), None);
        gate.transition(Some(3), || {});
        assert_eq!(gate.capture(|generation| generation), Some(3));
        assert_eq!(gate.capture_blocking(|generation| generation), Some(3));
        assert_eq!(gate.process(3, || 42), Some(42));
    }
}
