use crate::app::AppSingleton;
use crate::app::keyboard::AppKeyboard;
use crate::state::KeyboardObservable;
use anyhow::Result;
use nject::provider;
use tauri::AppHandle;

#[provider]
pub struct AppKeyboardProvider {
    #[provide(KeyboardObservable, |v| v.provide())]
    pub keyboard_observable: KeyboardObservable,

    #[provide(AppHandle, |h| h.clone())]
    pub handle: AppHandle,
}

impl AppSingleton for AppKeyboard {
    fn init(&self) -> Result<()> {
        // The supervisor starts the listeners as soon as permissions allow and
        // keeps them alive for the lifetime of the process.
        self.start_supervisor()
    }
}
