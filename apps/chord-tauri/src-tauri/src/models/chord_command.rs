use super::{ChordLayout, ChordMacroName, ChordMacros, Key};
use anyhow::{Context, Result};
use keycode::KeyMappingCode;

pub const CORE_CHORD_PACKAGE: &str = "@keychord/chords-chord";
pub const CORE_CHORD_REPO: &str = "KeyChord/chords-chord";
pub const CORE_CHORD_FILE: &str = "chords/com/leonsilicon/chord/macos.toml";

pub fn is_core_chords_file(package: &str, path: &std::path::Path) -> bool {
    package == CORE_CHORD_PACKAGE && path == std::path::Path::new(CORE_CHORD_FILE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaChordCommand {
    Settings,
    Reload,
    Quit,
    Halt(u32),
    SetLayout(ChordLayout),
    CycleLayout,
    SetMacro {
        name: ChordMacroName,
        sequence: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordRepeat {
    Times(u32),
    Loop,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChordCommand {
    Pending,
    /// All input starting with zero belongs to Chord, including unknown commands.
    Meta {
        keys: Vec<Key>,
    },
    Run {
        keys: Vec<Key>,
        repeat: ChordRepeat,
    },
}

impl ChordCommand {
    pub fn parse(input: &[Key], macros: &ChordMacros) -> Result<Self> {
        // Typed escape notation and physical arrow keys resolve to the same trigger.
        let sequence = Key::serialize_sequence(input).context("invalid chord sequence")?;
        if Key::validate_sequence_escapes(&sequence).is_err() {
            return Ok(Self::Pending);
        }
        let Some(sequence) = macros.expand(&sequence)? else {
            return Ok(Self::Pending);
        };
        let Ok(input) = Key::parse_sequence(&sequence) else {
            // A backslash introduces a special key or macro, never a literal chord.
            // Keep incomplete or unknown escapes editable without dispatching handlers.
            return Ok(Self::Pending);
        };
        if input.first() == Some(&Key(KeyMappingCode::Digit0)) {
            return Ok(Self::Meta { keys: input });
        }
        let prefix_length = input.iter().take_while(|key| key.is_digit()).count();
        if prefix_length == input.len() {
            return Ok(Self::Pending);
        }
        let repeat = if prefix_length == 0 {
            ChordRepeat::Times(1)
        } else {
            let count = Key::serialize_sequence(&input[..prefix_length])
                .context("invalid repeat prefix")?
                .parse::<u32>()
                .context("repeat count must fit in an unsigned 32-bit integer")?;
            if count == 1 {
                ChordRepeat::Loop
            } else {
                ChordRepeat::Times(count)
            }
        };
        Ok(Self::Run {
            keys: input[prefix_length..].to_vec(),
            repeat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<ChordCommand> {
        ChordCommand::parse(&Key::parse_sequence(input)?, &ChordMacros::default())
    }

    #[test]
    fn numeric_prefixes_do_not_consume_numeric_arguments() {
        for (input, command, repeat) in [
            ("hsr12", "hsr12", ChordRepeat::Times(1)),
            ("12hsr34", "hsr34", ChordRepeat::Times(12)),
            ("10ab", "ab", ChordRepeat::Times(10)),
            ("1ab", "ab", ChordRepeat::Loop),
        ] {
            assert_eq!(
                parse(input).unwrap(),
                ChordCommand::Run {
                    keys: Key::parse_sequence(command).unwrap(),
                    repeat,
                }
            );
        }
        assert_eq!(parse("12").unwrap(), ChordCommand::Pending);
        assert!(parse("4294967296ab").is_err());
    }

    #[test]
    fn zero_always_selects_the_reserved_namespace() {
        for input in ["0s", "0r", "0q", "0h12", "0", "0h", "0h12x", "00ab"] {
            assert_eq!(
                parse(input).unwrap(),
                ChordCommand::Meta {
                    keys: Key::parse_sequence(input).unwrap(),
                }
            );
        }
    }

    #[test]
    fn typed_special_keys_remain_pending_until_the_escape_is_complete() {
        for (word, arrow) in [
            ("up", KeyMappingCode::ArrowUp),
            ("left", KeyMappingCode::ArrowLeft),
            ("down", KeyMappingCode::ArrowDown),
            ("right", KeyMappingCode::ArrowRight),
        ] {
            let mut input = Key::parse_sequence("12a").unwrap();
            input.push(Key(KeyMappingCode::Backslash));
            for letter in word.chars() {
                assert_eq!(
                    ChordCommand::parse(&input, &ChordMacros::default()).unwrap(),
                    ChordCommand::Pending
                );
                input.push(letter.to_string().parse().unwrap());
            }
            let expected = ChordCommand::Run {
                keys: vec![Key(KeyMappingCode::KeyA), Key(arrow)],
                repeat: ChordRepeat::Times(12),
            };
            assert_eq!(
                ChordCommand::parse(&input, &ChordMacros::default()).unwrap(),
                expected
            );
            let mut physical = Key::parse_sequence("12a").unwrap();
            physical.push(Key(arrow));
            assert_eq!(
                ChordCommand::parse(&physical, &ChordMacros::default()).unwrap(),
                expected
            );
        }
        // Former system-action chords and unknown escapes never reach a handler.
        for prefix in ["", "0", "12a"] {
            for suffix in ["", "s", "q", "r", "unknown"] {
                let mut input = Key::parse_sequence(prefix).unwrap();
                input.push(Key(KeyMappingCode::Backslash));
                input.extend(Key::parse_sequence(suffix).unwrap());
                assert_eq!(
                    ChordCommand::parse(&input, &ChordMacros::default()).unwrap(),
                    ChordCommand::Pending
                );
            }
        }
    }
}
