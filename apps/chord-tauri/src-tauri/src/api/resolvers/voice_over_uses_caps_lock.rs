use crate::api::ApiImpl;
use crate::app::keyboard::voice_over;

pub async fn voice_over_uses_caps_lock(_api: ApiImpl) -> bool {
    voice_over::uses_caps_lock_modifier()
}
