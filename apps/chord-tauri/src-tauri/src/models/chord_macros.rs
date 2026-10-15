use super::Key;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{BTreeMap, HashMap};
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum ChordMacroName {
    Buffer(u8),
    Named(String),
}

impl ChordMacroName {
    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Named(name) if !name.starts_with(|c: char| c.is_ascii_digit()))
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Buffer(digit) => {
                anyhow::ensure!((1..=9).contains(digit), "buffer number must be 1–9")
            }
            Self::Named(name) => {
                anyhow::ensure!(
                    !name.is_empty() && !name.contains(['=', '\\']),
                    "macro names must be nonempty and cannot contain = or backslash"
                );
                Key::parse_sequence(name).context("invalid macro name")?;
            }
        }
        Ok(())
    }
}

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChordMacroEntry {
    pub target: ChordMacroName,
    pub sequence: String,
    pub persistent: bool,
}

/// Named macros persist unless their name starts with a digit. Buffers are temporary.
#[derive(Default, Clone)]
pub struct ChordMacros {
    definitions: HashMap<ChordMacroName, String>,
}

/// A numbered reference consumes two characters. A named reference requires a
/// closing `=`, which is consumed along with the name.
pub(crate) fn parse_macro_reference(input: &str) -> Option<(ChordMacroName, usize)> {
    let escaped = input.strip_prefix('\\')?;
    if let Some(digit @ b'1'..=b'9') = escaped.as_bytes().first() {
        return Some((ChordMacroName::Buffer(digit - b'0'), 2));
    }
    let named = escaped.strip_prefix('=')?;
    let end = named.find('=')?;
    let name = &named[..end];
    if name.is_empty() || name.contains('\\') {
        return None;
    }
    Some((ChordMacroName::Named(name.to_owned()), 3 + end))
}

impl ChordMacros {
    pub fn set(&mut self, name: ChordMacroName, sequence: String) {
        self.definitions.insert(name, sequence);
    }

    pub fn from_persistent(definitions: BTreeMap<String, String>) -> Result<Self> {
        let mut macros = Self::default();
        for (name, sequence) in definitions {
            let target = ChordMacroName::Named(name);
            if target.is_persistent() {
                target.validate()?;
                Key::parse_sequence(&sequence).context("invalid saved macro sequence")?;
                macros.set(target, sequence);
            }
        }
        Ok(macros)
    }

    pub fn persistent_definitions(&self) -> BTreeMap<String, String> {
        self.definitions
            .iter()
            .filter_map(|(target, sequence)| match target {
                ChordMacroName::Named(name) if target.is_persistent() => {
                    Some((name.clone(), sequence.clone()))
                }
                _ => None,
            })
            .collect()
    }

    pub fn entries(&self) -> Vec<ChordMacroEntry> {
        let mut entries = self
            .definitions
            .iter()
            .map(|(target, sequence)| ChordMacroEntry {
                target: target.clone(),
                sequence: sequence.clone(),
                persistent: target.is_persistent(),
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| match &entry.target {
            ChordMacroName::Buffer(digit) => (0, digit.to_string()),
            ChordMacroName::Named(name) => (1, name.clone()),
        });
        entries
    }

    /// Commit only after persistent changes have been saved successfully.
    pub fn update(
        &mut self,
        target: ChordMacroName,
        sequence: Option<String>,
        previous_target: Option<ChordMacroName>,
        save: impl FnOnce(&BTreeMap<String, String>) -> Result<()>,
    ) -> Result<()> {
        target.validate()?;
        let mut next = self.clone();
        if let Some(sequence) = sequence {
            Key::parse_sequence(&sequence)?;
            if let Some(previous) = previous_target.filter(|previous| previous != &target) {
                anyhow::ensure!(
                    next.definitions.contains_key(&previous),
                    "macro no longer exists"
                );
                anyhow::ensure!(
                    !next.definitions.contains_key(&target),
                    "a macro with that name already exists"
                );
                next.definitions.remove(&previous);
            }
            next.set(target, sequence);
        } else {
            next.definitions.remove(&target);
        }
        let persisted = next.persistent_definitions();
        if persisted != self.persistent_definitions() {
            save(&persisted)?;
        }
        *self = next;
        Ok(())
    }

    /// Missing definitions stay pending, just like incomplete special-key escapes.
    pub fn expand(&self, input: &str) -> Result<Option<String>> {
        let mut output = String::new();
        if self.expand_into(input, &mut Vec::new(), &mut output, &mut 4096)? {
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }

    fn expand_into(
        &self,
        mut input: &str,
        stack: &mut Vec<ChordMacroName>,
        output: &mut String,
        remaining_expansions: &mut usize,
    ) -> Result<bool> {
        while !input.is_empty() {
            if let Some((name, length)) = parse_macro_reference(input) {
                anyhow::ensure!(
                    *remaining_expansions > 0,
                    "chord exceeds 4096 macro expansions"
                );
                *remaining_expansions -= 1;
                let Some(sequence) = self.definitions.get(&name) else {
                    return Ok(false);
                };
                anyhow::ensure!(!stack.contains(&name), "recursive chord macro: {name:?}");
                anyhow::ensure!(stack.len() < 64, "chord macro nesting exceeds 64 levels");
                stack.push(name);
                let expanded = self.expand_into(sequence, stack, output, remaining_expansions)?;
                stack.pop();
                if !expanded {
                    return Ok(false);
                }
                input = &input[length..];
            } else {
                let character = input.chars().next().unwrap();
                output.push(character);
                input = &input[character.len_utf8()..];
            }
            anyhow::ensure!(output.len() <= 64 * 1024, "expanded chord exceeds 64 KiB");
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ChordCommand, ChordRepeat, Key};

    fn named(name: &str) -> ChordMacroName {
        ChordMacroName::Named(name.into())
    }

    #[test]
    fn persistence_survives_restart_and_digit_prefixed_names_remain_temporary() {
        let path = std::env::temp_dir().join(format!("chord-macros-{}.json", uuid::Uuid::new_v4()));
        let save = |definitions: &BTreeMap<String, String>| -> Result<()> {
            std::fs::write(&path, serde_json::to_vec(definitions)?)?;
            Ok(())
        };
        let mut macros = ChordMacros::default();
        macros
            .update(named("size"), Some("1720x864".into()), None, save)
            .unwrap();
        macros
            .update(named("alias"), Some(r"\=size=".into()), None, save)
            .unwrap();
        for digit in 0..=9 {
            macros
                .update(
                    named(&format!("{digit}size")),
                    Some("800x600".into()),
                    None,
                    |_| panic!("temporary names must not write to disk"),
                )
                .unwrap();
        }
        macros
            .update(
                ChordMacroName::Buffer(1),
                Some("640x480".into()),
                None,
                |_| panic!("buffers must not write to disk"),
            )
            .unwrap();
        let saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let restarted = ChordMacros::from_persistent(saved).unwrap();
        assert_eq!(
            restarted.expand(r"/ws\=alias=").unwrap().as_deref(),
            Some("/ws1720x864")
        );
        assert!(restarted.expand(r"\1").unwrap().is_none());
        for digit in 0..=9 {
            assert!(
                restarted
                    .expand(&format!(r"\={digit}size="))
                    .unwrap()
                    .is_none()
            );
        }
        // Renaming to a digit prefix removes the persisted definition.
        macros
            .update(
                named("1size"),
                Some("1720x864".into()),
                Some(named("size")),
                save,
            )
            .unwrap_err();
        macros
            .update(
                named("2temporary"),
                Some("1720x864".into()),
                Some(named("size")),
                save,
            )
            .unwrap();
        let saved = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let restarted = ChordMacros::from_persistent(saved).unwrap();
        assert!(restarted.expand(r"\=size=").unwrap().is_none());
        assert!(restarted.expand(r"\=2temporary=").unwrap().is_none());
        // Renaming back persists it, including later overwrites and deletion.
        macros
            .update(
                named("size"),
                Some("1024x768".into()),
                Some(named("2temporary")),
                save,
            )
            .unwrap();
        macros
            .update(named("size"), Some("800x600".into()), None, save)
            .unwrap();
        let restarted = ChordMacros::from_persistent(
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            restarted.expand(r"\=size=").unwrap().as_deref(),
            Some("800x600")
        );
        macros.update(named("size"), None, None, save).unwrap();
        let restarted = ChordMacros::from_persistent(
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(restarted.expand(r"\=size=").unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_persistence_and_invalid_edits_preserve_the_current_definitions() {
        let mut macros = ChordMacros::default();
        macros.set(named("size"), "1720x864".into());
        for (target, value, previous) in [
            (named("size"), Some("800x600".into()), None),
            (named("size"), None, None),
            (named("1size"), Some("800x600".into()), Some(named("size"))),
        ] {
            assert!(
                macros
                    .update(target, value, previous, |_| anyhow::bail!("disk full"))
                    .is_err()
            );
            assert_eq!(
                macros.expand(r"\=size=").unwrap().as_deref(),
                Some("1720x864")
            );
        }
        for (target, sequence) in [
            (named(""), "ab"),
            (named("bad=name"), "ab"),
            (named("valid"), r"\=incomplete"),
            (ChordMacroName::Buffer(0), "ab"),
        ] {
            assert!(
                macros
                    .update(target, Some(sequence.into()), None, |_| panic!(
                        "invalid edits must not be persisted"
                    ))
                    .is_err()
            );
        }
    }

    #[test]
    fn references_round_trip_and_named_references_require_a_closing_equals() {
        for input in [
            r"\1\2\3\4\5\6\7\8\9",
            r"/ws\=window-size=",
            r"a\=name=b",
            r"\=1=",
        ] {
            let keys = Key::parse_sequence(input).unwrap();
            assert_eq!(Key::serialize_sequence(&keys).unwrap(), input);
        }
        for input in [
            r"\0",
            r"\=",
            r"\==",
            r"\=name\1",
            r"\=name",
            r"/ws\=window-size",
        ] {
            assert!(Key::parse_sequence(input).is_err(), "{input}");
        }
        let mut macros = ChordMacros::default();
        macros.set(ChordMacroName::Buffer(1), "1720x864".into());
        macros.set(named("window-size"), r"\1".into());
        for input in [r"/ws\1", r"/ws\=window-size="] {
            assert_eq!(
                macros.expand(input).unwrap().as_deref(),
                Some("/ws1720x864")
            );
        }
        assert_eq!(
            macros.expand(r"a\=window-size=b").unwrap().as_deref(),
            Some("a1720x864b")
        );
        macros.set(named("1"), "named".into());
        assert_eq!(
            macros.expand(r"\=1=\1").unwrap().as_deref(),
            Some("named1720x864")
        );
        macros.set(ChordMacroName::Buffer(1), "800x600".into());
        assert_eq!(
            macros.expand(r"\=window-size=").unwrap().as_deref(),
            Some("800x600")
        );
        macros.set(ChordMacroName::Buffer(1), String::new());
        assert_eq!(macros.expand(r"a\1b").unwrap().as_deref(), Some("ab"));
    }

    #[test]
    fn expansion_precedes_repetition_and_can_select_core_commands() {
        let mut macros = ChordMacros::default();
        macros.set(named("repeat"), "12".into());
        macros.set(named("settings"), "0,".into());
        macros.set(ChordMacroName::Buffer(1), r"a\up".into());
        assert_eq!(
            ChordCommand::parse(&Key::parse_sequence(r"\=repeat=\1").unwrap(), &macros).unwrap(),
            ChordCommand::Run {
                keys: Key::parse_sequence(r"a\up").unwrap(),
                repeat: ChordRepeat::Times(12)
            }
        );
        assert_eq!(
            ChordCommand::parse(&Key::parse_sequence(r"\=settings=").unwrap(), &macros).unwrap(),
            ChordCommand::Meta {
                keys: Key::parse_sequence("0,").unwrap()
            }
        );
        macros.set(ChordMacroName::Buffer(2), "12".into());
        assert_eq!(
            ChordCommand::parse(&Key::parse_sequence(r"0h\2").unwrap(), &macros).unwrap(),
            ChordCommand::Meta {
                keys: Key::parse_sequence("0h12").unwrap()
            }
        );
    }

    #[test]
    fn undefined_references_stay_pending_and_recursive_or_huge_expansions_fail() {
        let mut macros = ChordMacros::default();
        for input in [r"a\1", r"a\=missing=", r"a\=missing", r"a\="] {
            // Physical typing also permits an incomplete trailing escape.
            let keys = input
                .chars()
                .map(|c| c.to_string().parse().unwrap())
                .collect::<Vec<Key>>();
            assert_eq!(
                ChordCommand::parse(&keys, &macros).unwrap(),
                ChordCommand::Pending
            );
        }
        macros.set(ChordMacroName::Buffer(1), r"\=cycle=".into());
        macros.set(named("cycle"), r"\1".into());
        assert!(
            macros
                .expand(r"\1")
                .unwrap_err()
                .to_string()
                .contains("recursive")
        );
        macros.set(named("large"), "a".repeat(64 * 1024 + 1));
        assert!(
            macros
                .expand(r"\=large=")
                .unwrap_err()
                .to_string()
                .contains("64 KiB")
        );
        // Even an acyclic expansion with empty output must have bounded work.
        macros.set(named("empty0"), String::new());
        for index in 1..=13 {
            let child = format!(r"\=empty{}=", index - 1);
            macros.set(named(&format!("empty{index}")), child.repeat(2));
        }
        assert!(
            macros
                .expand(r"\=empty13=")
                .unwrap_err()
                .to_string()
                .contains("4096")
        );
    }
}
