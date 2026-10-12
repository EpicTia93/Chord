use super::{Key, KeyEvent};
use serde::Serialize;
use std::sync::OnceLock;
use std::time::Instant;
use typeshare::typeshare;

/// The KeyboardEvent data exposed as a JavaScript handler's `this.event`.
#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordKeyboardEvent {
    pub r#type: String,
    pub key: String,
    pub code: String,
    pub location: u32,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
    pub repeat: bool,
    pub is_composing: bool,
    pub time_stamp: f64,
}

impl ChordKeyboardEvent {
    pub fn new(event: &KeyEvent, held_keys: &[Key], repeat: bool) -> Self {
        use keycode::KeyMappingCode::*;
        let (KeyEvent::Press(key) | KeyEvent::Release(key)) = event;
        let has =
            |codes: &[keycode::KeyMappingCode]| held_keys.iter().any(|k| codes.contains(&k.0));
        let shift_key = has(&[ShiftLeft, ShiftRight]);
        let code = key.0.to_string();
        let key_value = match key.0 {
            ShiftLeft | ShiftRight => "Shift".to_string(),
            ControlLeft | ControlRight => "Control".to_string(),
            AltLeft | AltRight => "Alt".to_string(),
            MetaLeft | MetaRight => "Meta".to_string(),
            Space => " ".to_string(),
            NumpadEnter => "Enter".to_string(),
            NumpadBackspace => "Backspace".to_string(),
            NumpadAdd => "+".to_string(),
            NumpadSubtract => "-".to_string(),
            NumpadMultiply => "*".to_string(),
            NumpadDivide => "/".to_string(),
            NumpadDecimal => ".".to_string(),
            NumpadEqual => "=".to_string(),
            Numpad0 | Numpad1 | Numpad2 | Numpad3 | Numpad4 | Numpad5 | Numpad6 | Numpad7
            | Numpad8 | Numpad9 => code.trim_start_matches("Numpad").to_string(),
            _ => key
                .to_char(shift_key)
                .map(|c| c.to_string())
                .unwrap_or_else(|| code.clone()),
        };
        let location = if code.starts_with("Numpad") {
            3
        } else if matches!(key.0, ShiftLeft | ControlLeft | AltLeft | MetaLeft) {
            1
        } else if matches!(key.0, ShiftRight | ControlRight | AltRight | MetaRight) {
            2
        } else {
            0
        };
        static TIME_ORIGIN: OnceLock<Instant> = OnceLock::new();
        Self {
            r#type: if matches!(event, KeyEvent::Press(_)) {
                "keydown"
            } else {
                "keyup"
            }
            .into(),
            key: key_value,
            code,
            location,
            ctrl_key: has(&[ControlLeft, ControlRight]),
            shift_key,
            alt_key: has(&[AltLeft, AltRight]),
            meta_key: has(&[MetaLeft, MetaRight]),
            repeat: repeat && matches!(event, KeyEvent::Press(_)),
            is_composing: false,
            time_stamp: TIME_ORIGIN
                .get_or_init(Instant::now)
                .elapsed()
                .as_secs_f64()
                * 1_000.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycode::KeyMappingCode::*;

    #[test]
    fn keyboard_event_uses_web_names_and_modifier_fields() {
        let modifiers = [
            Key(ShiftLeft),
            Key(ControlRight),
            Key(AltLeft),
            Key(MetaRight),
        ];
        let event = ChordKeyboardEvent::new(&KeyEvent::Press(Key(Backspace)), &modifiers, true);
        assert_eq!(event.key, "Backspace");
        assert_eq!(event.code, "Backspace");
        assert!(
            event.repeat && event.shift_key && event.ctrl_key && event.alt_key && event.meta_key
        );
        let serialized = serde_json::to_value(event).unwrap();
        assert_eq!(serialized["type"], "keydown");
        assert_eq!(serialized["isComposing"], false);
        assert!(serialized["timeStamp"].is_number());
        assert_eq!(
            ChordKeyboardEvent::new(&KeyEvent::Press(Key(KeyA)), &modifiers, false).key,
            "A"
        );
        let up = ChordKeyboardEvent::new(&KeyEvent::Release(Key(ShiftRight)), &[], true);
        assert_eq!(up.key, "Shift");
        assert_eq!(up.location, 2);
        assert_eq!(up.r#type, "keyup");
        assert!(!up.repeat && !up.shift_key);
        let numpad = ChordKeyboardEvent::new(&KeyEvent::Press(Key(Numpad3)), &[], false);
        assert_eq!(numpad.key, "3");
        assert_eq!(numpad.code, "Numpad3");
        assert_eq!(numpad.location, 3);
    }
}
