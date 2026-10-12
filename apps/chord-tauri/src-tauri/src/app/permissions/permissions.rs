use crate::state::{AppPermissionsObservable, AppPermissionsState, Observable};
use anyhow::{Context, Result};
use nject::injectable;
use std::sync::mpsc;
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

#[injectable]
pub struct AppPermissions {
    observable: AppPermissionsObservable,
    handle: AppHandle,
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

    /// Queries macOS directly: `(accessibility, input_monitoring)`.
    pub fn check_now() -> (bool, bool) {
        (
            macos_accessibility_client::accessibility::application_is_trusted(),
            input_monitoring_granted(),
        )
    }

    /// Re-checks both permissions and publishes them only when they changed.
    /// Cheap enough to poll: both calls are local TCC lookups.
    pub fn refresh_if_changed(&self) -> Result<(bool, bool)> {
        let (accessibility, input_monitoring) = Self::check_now();
        let previous = self.observable.get_state()?;
        let changed = previous.is_accessibility_enabled != Some(accessibility)
            || previous.is_input_monitoring_enabled != Some(input_monitoring);
        if changed {
            log::info!(
                "Permissions: accessibility={accessibility} input_monitoring={input_monitoring} (executable {:?})",
                std::env::current_exe().ok()
            );
            self.observable.set_state(|prev| AppPermissionsState {
                is_input_monitoring_enabled: Some(input_monitoring),
                is_accessibility_enabled: Some(accessibility),
                ..prev
            })?;
        }
        Ok((accessibility, input_monitoring))
    }

    pub async fn load(&self) -> Result<(bool, bool)> {
        self.refresh_if_changed()
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
/// `kIOHIDAccessTypeGranted`
const IOHID_ACCESS_TYPE_GRANTED: u32 = 0;

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDRequestAccess(request: u32) -> u8;
    fn IOHIDCheckAccess(request: u32) -> u32;
}

#[cfg(target_os = "macos")]
fn input_monitoring_granted() -> bool {
    // SAFETY: plain IOKit query without pointers.
    unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) == IOHID_ACCESS_TYPE_GRANTED }
}

#[cfg(not(target_os = "macos"))]
fn input_monitoring_granted() -> bool {
    true
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
