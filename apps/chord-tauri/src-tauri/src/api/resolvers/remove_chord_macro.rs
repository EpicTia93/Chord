use crate::api::{ApiImpl, AppResult};
use crate::app::AppHandleExt;
use crate::models::ChordMacroName;

pub async fn remove_chord_macro(api: ApiImpl, target: ChordMacroName) -> AppResult<()> {
    api.handle()?
        .app_state()
        .chord_mode_manager()
        .update_macro(target, None, None)?;
    Ok(())
}
