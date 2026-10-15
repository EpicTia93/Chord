//! Native AX notifications update cached context; input dispatch never queries AX.
use crate::app::AppHandleExt;
use crate::state::AppPermissionsState;
use anyhow::Result;
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Manager};

static HANDLE: OnceLock<AppHandle> = OnceLock::new();

unsafe extern "C" {
    fn chord_file_picker_set_enabled(enabled: bool, changed: extern "C" fn(bool));
}

extern "C" fn changed(active: bool) {
    // Never unwind through an AppKit/AX callback, including if a subscriber panics.
    let _ = std::panic::catch_unwind(|| {
        if let Some(handle) = HANDLE.get() {
            if let Err(error) = handle
                .app_state()
                .frontmost()
                .set_file_picker_active(active)
            {
                log::error!("Failed to update file picker context: {error:#}");
            }
        }
    });
}

fn set_enabled(handle: &AppHandle, enabled: bool) -> Result<()> {
    handle.run_on_main_thread(move || unsafe {
        chord_file_picker_set_enabled(enabled, changed);
    })?;
    Ok(())
}

pub(super) fn init(handle: AppHandle) -> Result<()> {
    if HANDLE.set(handle.clone()).is_err() {
        return Ok(());
    }
    let permissions =
        handle.state::<observable_property::ObservableProperty<AppPermissionsState>>();
    let observer_handle = handle.clone();
    permissions.subscribe(Arc::new(move |previous, current| {
        if previous.is_accessibility_enabled != current.is_accessibility_enabled {
            if let Err(error) = set_enabled(
                &observer_handle,
                current.is_accessibility_enabled == Some(true),
            ) {
                log::error!("Failed to update file picker listener permissions: {error:#}");
            }
        }
    }))?;
    set_enabled(
        &handle,
        permissions.get()?.is_accessibility_enabled == Some(true),
    )
}
