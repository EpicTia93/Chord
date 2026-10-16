use super::DevLockfileDetector;
use super::lease::{InputLease, is_development};
use crate::app::{AppHandleExt, AppSingleton};
use anyhow::Result;
use nject::provider;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// How often standby behind another instance is repeated in the log.
const WAITING_REPORT_INTERVAL: Duration = Duration::from_secs(60);

#[provider]
pub struct DevLockfileDetectorProvider {
    #[provide(AppHandle, |h| h.clone())]
    pub handle: AppHandle,
}

impl AppSingleton for DevLockfileDetector {
    fn init(&self) -> Result<()> {
        // local_data_dir is user-scoped, not bundle-scoped: every channel must
        // open the same inodes, even with a different TMPDIR or bundle suffix.
        let directory = self
            .handle
            .path()
            .local_data_dir()?
            .join("com.leonsilicon.chord/input-ownership-v1");
        let mut lease =
            InputLease::open(&directory, is_development(&self.handle.config().identifier))?;
        let handle = self.handle.clone();
        std::thread::Builder::new()
            .name("chord-input-owner".into())
            .spawn(move || {
                let mut next_generation = 0;
                let mut changed = |active| {
                    let detector = handle.app_state().dev_lockfile_detector();
                    let next = if active {
                        next_generation += 1;
                        Some(next_generation)
                    } else {
                        None
                    };
                    detector.gate.transition(next, || {
                        handle.app_state().keyboard().reset_input_state();
                        if let Err(error) = handle.app_state().app_controller().set_idle_mode() {
                            log::error!(
                                "Failed to clear input state during ownership handoff: {error:#}"
                            );
                        }
                    });
                    // Before the lease is released, so the next owner takes the Caps
                    // key only after this one gave it back.
                    handle.app_state().keyboard().set_input_owner(active);
                    log::info!(
                        "Input ownership: {} ({})",
                        if active { "active" } else { "standby" },
                        handle.config().identifier
                    );
                };
                let mut waiting_since: Option<Instant> = None;
                let mut last_waiting_report: Option<Instant> = None;
                loop {
                    let eligible = handle.app_state().permissions().can_intercept_input()
                        && handle.app_state().keyboard().input_handlers_running();
                    let owns_input = match lease.refresh(eligible, &mut changed) {
                        Ok(owns_input) => owns_input,
                        Err(error) => {
                            // Fail closed. Drop releases both leases only after the gate
                            // has been closed; another process can safely take over.
                            changed(false);
                            log::error!("Input ownership failed; interception disabled: {error:#}");
                            break;
                        }
                    };
                    // Standby behind another instance is otherwise invisible: the app
                    // looks active while a dev build (or a stuck one) owns input.
                    if eligible && !owns_input {
                        let now = Instant::now();
                        let since = *waiting_since.get_or_insert(now);
                        if last_waiting_report.is_none_or(|last| {
                            now.duration_since(last) >= WAITING_REPORT_INTERVAL
                        }) {
                            last_waiting_report = Some(now);
                            log::warn!(
                                "Input ownership: waiting ({}); another Chord instance owns input (for {:?})",
                                handle.config().identifier,
                                now.duration_since(since)
                            );
                        }
                    } else {
                        waiting_since = None;
                        last_waiting_report = None;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            })?;
        Ok(())
    }
}
