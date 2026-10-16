//! Native AX notifications update cached context; input dispatch never queries AX.
use crate::app::AppHandleExt;
use crate::state::AppPermissionsState;
use anyhow::Result;
use objc2::MainThreadMarker;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, OnceLock},
};
use tauri::{AppHandle, Manager};

mod native;

thread_local! {
    static LISTENER: RefCell<Option<Rc<native::Listener>>> = const { RefCell::new(None) };
}

fn set_enabled(handle: &AppHandle, enabled: bool) -> Result<()> {
    let callback_handle = handle.clone();
    handle.run_on_main_thread(move || {
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        if enabled {
            LISTENER.with(|slot| {
                if slot.borrow().is_none() {
                    let listener = native::Listener::start(main_thread, move |active| {
                        if let Err(error) = callback_handle
                            .app_state()
                            .frontmost()
                            .set_file_picker_active(active)
                        {
                            log::error!("Failed to update file picker context: {error:#}");
                        }
                    });
                    *slot.borrow_mut() = Some(listener);
                }
            });
        } else {
            // Drop outside the RefCell borrow: Drop publishes the inactive state.
            drop(LISTENER.with(|slot| slot.borrow_mut().take()));
            let _ = callback_handle
                .app_state()
                .frontmost()
                .set_file_picker_active(false);
        }
    })?;
    Ok(())
}

pub(super) fn init(handle: AppHandle) -> Result<()> {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    if INITIALIZED.set(()).is_err() {
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
