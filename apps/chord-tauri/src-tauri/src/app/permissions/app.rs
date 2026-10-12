use crate::app::AppSingleton;
use crate::app::permissions::AppPermissions;
use crate::state::AppPermissionsObservable;
use anyhow::Result;
use nject::provider;
use tauri::AppHandle;

#[provider]
pub struct AppPermissionsProvider {
    #[provide(AppPermissionsObservable, |v| v.provide())]
    pub app_permissions_observable: AppPermissionsObservable,

    #[provide(AppHandle, |h| h.clone())]
    pub handle: AppHandle,
}

impl AppSingleton for AppPermissions {
    fn init(&self) -> Result<()> {
        self.init()
    }
}
