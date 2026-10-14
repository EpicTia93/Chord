use super::{ChordKeyboardEvent, ChordTrigger, Key, MetaChordCommand, SimulatedShortcut};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    Builtin(BuiltinChordAction),
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
            Self::Shortcut(_) | Self::Shell(_) | Self::Builtin(_) => ChordEventTrigger::Input,
        }
    }
}

#[typeshare]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltinChordKind {
    Settings,
    Reload,
    Quit,
    HaltLoop,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinChordAction {
    pub command: BuiltinChordKind,
    pub argument: Option<String>,
}

impl BuiltinChordAction {
    pub fn resolve(&self, trigger: &ChordTrigger, keys: &[Key]) -> Result<MetaChordCommand> {
        Ok(match self.command {
            BuiltinChordKind::Settings => MetaChordCommand::Settings,
            BuiltinChordKind::Reload => MetaChordCommand::Reload,
            BuiltinChordKind::Quit => MetaChordCommand::Quit,
            BuiltinChordKind::HaltLoop => {
                let argument = self
                    .argument
                    .as_deref()
                    .context("halt-loop requires an argument")?;
                let input = Key::serialize_sequence(keys).context("invalid meta chord")?;
                let value = if let Some(capture) = argument.strip_prefix('$') {
                    let ChordTrigger::Pattern(pattern) = trigger else {
                        anyhow::bail!("halt-loop capture requires a regex trigger");
                    };
                    let captures = pattern
                        .captures(&input)
                        .context("meta chord did not match")?;
                    captures
                        .get(
                            capture
                                .parse::<usize>()
                                .context("invalid loop ID capture")?,
                        )
                        .context("loop ID capture not found")?
                        .as_str()
                        .to_owned()
                } else {
                    argument.to_owned()
                };
                MetaChordCommand::Halt(value.parse().context("invalid loop ID")?)
            }
        })
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandlerChordAction {
    /// Key in the JS engine's `__RUST_HANDLER_REGISTRY`.
    pub handler_id: String,
    #[typeshare(typescript(type = "any[]"))]
    pub event_args: Vec<toml::Value>,
    /// Entered chord sequence, available as `this.input` in the handler.
    #[serde(default)]
    pub input: String,
    /// Persistent JavaScript values for this chord's current matching interval.
    pub sequence_id: Option<String>,
    pub assign_to: Option<String>,
    pub keyboard_event: Option<ChordKeyboardEvent>,
}
