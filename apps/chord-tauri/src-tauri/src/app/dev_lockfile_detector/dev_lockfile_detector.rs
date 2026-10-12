use super::gate::InputGate;
use nject::injectable;
use tauri::AppHandle;

#[injectable]
pub struct DevLockfileDetector {
    // Ownership changes wait for callbacks and queued handlers to finish before
    // releasing the OS lease. Queued events retain their originating generation.
    #[inject(InputGate::default())]
    pub(super) gate: InputGate,
    pub(super) handle: AppHandle,
}

impl DevLockfileDetector {
    /// Non-blocking: returns `None` while a handoff is in progress. For the
    /// event tap, which must answer within the Core Graphics deadline.
    pub fn with_input_owner<T>(&self, f: impl FnOnce(u64) -> T) -> Option<T> {
        self.gate.capture(f)
    }

    /// Waits for any handoff to finish, then runs `f` if this process owns input.
    pub fn with_current_input_owner<T>(&self, f: impl FnOnce(u64) -> T) -> Option<T> {
        self.gate.capture_blocking(f)
    }

    pub fn with_input_generation<T>(&self, expected: u64, f: impl FnOnce() -> T) -> Option<T> {
        self.gate.process(expected, f)
    }
}
