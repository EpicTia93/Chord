use crate::state::{AppPermissionsObservable, AppPermissionsState, Observable};
use anyhow::{Context, Result};
use nject::injectable;
use std::sync::{Mutex, mpsc};
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

#[injectable]
pub struct AppPermissions {
    observable: AppPermissionsObservable,
    handle: AppHandle,
    #[inject(Mutex::new(None))]
    last_input_monitoring: Mutex<Option<InputMonitoringAccess>>,
    #[inject(ExecutableIdentity::capture())]
    executable: ExecutableIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMonitoringAccess {
    Granted,
    Denied,
    Unknown,
    Unexpected(u32),
}

impl InputMonitoringAccess {
    fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::Granted,
            1 => Self::Denied,
            2 => Self::Unknown,
            other => Self::Unexpected(other),
        }
    }

    pub fn is_granted(self) -> bool {
        self == Self::Granted
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputPermissions {
    pub accessibility: bool,
    pub input_monitoring: InputMonitoringAccess,
}

impl InputPermissions {
    pub const UNAVAILABLE: Self = Self {
        accessibility: false,
        input_monitoring: InputMonitoringAccess::Unknown,
    };
}

struct ExecutableIdentity {
    path: Option<std::path::PathBuf>,
    #[cfg(unix)]
    inode: Option<(u64, u64)>,
}

impl ExecutableIdentity {
    fn capture() -> Self {
        let path = std::env::current_exe().ok();
        #[cfg(unix)]
        let inode = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|metadata| {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            });
        Self {
            path,
            #[cfg(unix)]
            inode,
        }
    }

    fn invalidation(&self) -> Option<String> {
        if let Err(error) = std::env::current_exe() {
            return Some(format!(
                "Running executable path is unavailable ({error}); a rebuild may have unlinked it. Restart Chord from the development snapshot runner"
            ));
        }
        let path = self.path.as_ref()?;
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Some(format!(
                    "Running executable disappeared at {path:?}: {error}; restart Chord"
                ));
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if self
                .inode
                .is_some_and(|previous| previous != (metadata.dev(), metadata.ino()))
            {
                return Some(format!(
                    "Running executable was replaced at {path:?}; restart Chord from the development snapshot runner"
                ));
            }
        }
        None
    }
}

impl AppPermissions {
    pub(super) fn init(&self) -> Result<()> {
        // The input supervisor (`AppKeyboard`) polls `refresh_if_changed` and
        // owns listener registration; nothing to subscribe to here.
        Ok(())
    }

    pub fn can_intercept_input(&self) -> bool {
        self.observable.get_state().is_ok_and(|state| {
            state.is_accessibility_enabled == Some(true)
                && state.is_input_monitoring_enabled == Some(true)
        })
    }

    /// Queries macOS without collapsing undecided or unexpected results into denial.
    pub fn check_now() -> InputPermissions {
        InputPermissions {
            accessibility: macos_accessibility_client::accessibility::application_is_trusted(),
            input_monitoring: input_monitoring_access(),
        }
    }

    /// Re-checks both permissions and publishes them only when they changed.
    /// Cheap enough to poll: both calls are local TCC lookups.
    pub fn refresh_if_changed(&self) -> Result<InputPermissions> {
        let permissions = Self::check_now();
        let accessibility = permissions.accessibility;
        let input_monitoring = permissions.input_monitoring.is_granted();
        let previous = self.observable.get_state()?;
        let changed = previous.is_accessibility_enabled != Some(accessibility)
            || previous.is_input_monitoring_enabled != Some(input_monitoring);
        let status_changed = {
            let mut status = self
                .last_input_monitoring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let changed = *status != Some(permissions.input_monitoring);
            *status = Some(permissions.input_monitoring);
            changed
        };
        if changed || status_changed {
            log::info!(
                "Permissions: accessibility={accessibility} input_monitoring={:?} (executable {:?})",
                permissions.input_monitoring,
                std::env::current_exe().ok()
            );
            if (!accessibility || !input_monitoring)
                && let Some(reason) = self.executable.invalidation()
            {
                log::warn!("{reason}");
            }
        }
        if changed {
            self.observable.set_state(|prev| AppPermissionsState {
                is_input_monitoring_enabled: Some(input_monitoring),
                is_accessibility_enabled: Some(accessibility),
                ..prev
            })?;
        }
        Ok(permissions)
    }

    pub async fn load(&self) -> Result<(bool, bool)> {
        let permissions = self.refresh_if_changed()?;
        Ok((
            permissions.accessibility,
            permissions.input_monitoring.is_granted(),
        ))
    }

    pub fn toggle_autostart(&self) -> Result<()> {
        let state = self.observable.get_state()?;
        let Some(is_autostart_enabled) = state.is_autostart_enabled else {
            anyhow::bail!("app not ready")
        };
        if is_autostart_enabled {
            self.handle.autolaunch().disable()?;
            self.observable.set_state(|prev| AppPermissionsState {
                is_autostart_enabled: Some(false),
                ..prev
            })?;
        } else {
            self.handle.autolaunch().enable()?;
            self.observable.set_state(|prev| AppPermissionsState {
                is_autostart_enabled: Some(true),
                ..prev
            })?;
        }
        Ok(())
    }
}

pub fn open_system_settings(url: &str, permission_name: &str) {
    if let Err(error) = std::process::Command::new("open").arg(url).spawn() {
        log::error!("Failed to open {permission_name} settings: {error}");
    }
}

fn run_on_main_thread_sync<R: Runtime, F>(handle: &AppHandle<R>, f: F) -> Result<()>
where
    F: FnOnce() + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(0);
    handle
        .run_on_main_thread(move || {
            f();
            let _ = tx.send(());
        })
        .context("schedule main-thread dispatch")?;
    rx.recv()
        .context("main thread finished without signaling (channel closed)")?;
    Ok(())
}

/// `AXIsProcessTrustedWithOptions` must run on the main thread; otherwise the prompt can appear but
/// TCC may not associate the app correctly with the bundle UI in System Settings.
#[cfg(target_os = "macos")]
pub fn request_accessibility_prompt_sync<R: Runtime>(handle: &AppHandle<R>) -> Result<()> {
    log::info!(
        "Requesting accessibility trust for executable {:?}",
        std::env::current_exe().ok()
    );
    run_on_main_thread_sync(handle, || {
        macos_accessibility_client::accessibility::application_is_trusted_with_prompt();
    })
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility_prompt_sync<R: Runtime>(_handle: &AppHandle<R>) -> Result<()> {
    Ok(())
}

/// `kIOHIDRequestTypeListenEvent`
const IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDRequestAccess(request: u32) -> u8;
    fn IOHIDCheckAccess(request: u32) -> u32;
}

#[cfg(target_os = "macos")]
fn input_monitoring_access() -> InputMonitoringAccess {
    // SAFETY: plain IOKit query without pointers.
    InputMonitoringAccess::from_raw(unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) })
}

#[cfg(not(target_os = "macos"))]
fn input_monitoring_access() -> InputMonitoringAccess {
    InputMonitoringAccess::Granted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undecided_and_unexpected_access_fail_closed_without_claiming_denial() {
        assert_eq!(
            InputMonitoringAccess::from_raw(1),
            InputMonitoringAccess::Denied
        );
        assert_eq!(
            InputMonitoringAccess::from_raw(2),
            InputMonitoringAccess::Unknown
        );
        assert_eq!(
            InputMonitoringAccess::from_raw(99),
            InputMonitoringAccess::Unexpected(99)
        );
        for raw in [1, 2, 99] {
            assert!(!InputMonitoringAccess::from_raw(raw).is_granted());
        }
        assert!(InputMonitoringAccess::from_raw(0).is_granted());
    }
}

/// Registers this process for Input Monitoring (`kIOHIDRequestTypeListenEvent`) on the main thread.
pub fn register_input_monitoring_sync<R: Runtime>(handle: &AppHandle<R>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_on_main_thread_sync(handle, || unsafe {
            IOHIDRequestAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT);
        })?;
    }
    Ok(())
}
