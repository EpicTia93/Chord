use super::{ChordKeyboardEvent, SimulatedShortcut};
use serde::Serialize;
use typeshare::typeshare;

/// The action that a chord can define.
#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type", content = "content")]
pub enum ChordAction {
    Shortcut(ShortcutChordAction),
    Shell(ShellChordAction),
    Emit(EmitChordAction),
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChordEventTrigger {
    Input,
    Init,
    Keydown,
    Keyup,
    Keypress,
}

impl ChordAction {
    pub fn trigger(&self) -> ChordEventTrigger {
        match self {
            Self::Emit(action) => action.trigger,
            Self::Shortcut(_) | Self::Shell(_) => ChordEventTrigger::Input,
        }
    }
}

/// The action that a chord task is meant to execute.
#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type", content = "content")]
pub enum ChordTaskAction {
    Shortcut(ShortcutChordAction),
    Shell(ShellChordAction),
    Handler(HandlerChordAction),
}

#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutChordAction {
    pub simulated_shortcut: SimulatedShortcut,
}

#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellChordAction {
    pub command: String,
}

#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmitChordAction {
    pub trigger: ChordEventTrigger,
    pub event_key: String,
    pub assign_to: Option<String>,
    #[typeshare(typescript(type = "any"))]
    pub args: Vec<toml::Value>,
}

#[typeshare]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandlerChordAction {
    /// Key in the JS engine's `__RUST_HANDLER_REGISTRY`.
    pub handler_id: String,
    #[typeshare(typescript(type = "any[]"))]
    pub event_args: Vec<toml::Value>,
    /// Persistent JavaScript values for this chord's current matching interval.
    pub sequence_id: Option<String>,
    pub assign_to: Option<String>,
    pub keyboard_event: Option<ChordKeyboardEvent>,
}
