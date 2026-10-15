use crate::api::{ApiImpl, AppResult};
use crate::app::AppHandleExt;
use crate::models::ChordMacroName;

pub async fn set_chord_macro(
    api: ApiImpl,
    target: ChordMacroName,
    sequence: String,
    previous_target: Option<ChordMacroName>,
) -> AppResult<()> {
    api.handle()?
        .app_state()
        .chord_mode_manager()
        .update_macro(target, Some(sequence), previous_target)?;
    Ok(())
}
