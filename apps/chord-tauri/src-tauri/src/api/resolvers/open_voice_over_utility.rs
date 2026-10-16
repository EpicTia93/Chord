use crate::api::ApiImpl;
use crate::app::keyboard::voice_over::UTILITY_BUNDLE_ID;

pub async fn open_voice_over_utility(_api: ApiImpl) {
    if let Err(error) = std::process::Command::new("open")
        .args(["-b", UTILITY_BUNDLE_ID])
        .spawn()
    {
        log::error!("Failed to open VoiceOver Utility: {error}");
    }
}
