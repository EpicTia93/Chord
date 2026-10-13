use super::Key;
use anyhow::{Context, Result};
use keycode::KeyMappingCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaChordCommand {
    Settings,
    Reload,
    Quit,
    Halt(u32),
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
    Meta(Option<MetaChordCommand>),
    Run {
        keys: Vec<Key>,
        repeat: ChordRepeat,
    },
}

impl ChordCommand {
    pub fn parse(input: &[Key]) -> Result<Self> {
        if input.first() == Some(&Key(KeyMappingCode::Digit0)) {
            let sequence = Key::serialize_sequence(input).context("invalid meta chord")?;
            let command = match sequence.as_str() {
                "0s" => Some(MetaChordCommand::Settings),
                "0r" => Some(MetaChordCommand::Reload),
                "0q" => Some(MetaChordCommand::Quit),
                _ => match sequence.strip_prefix("0h") {
                    Some(id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => Some(
                        MetaChordCommand::Halt(id.parse().context("invalid loop ID")?),
                    ),
                    _ => None,
                },
            };
            return Ok(Self::Meta(command));
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
        // Typed escape notation and physical arrow keys resolve to the same trigger.
        let sequence =
            Key::serialize_sequence(&input[prefix_length..]).context("invalid chord sequence")?;
        Ok(Self::Run {
            keys: Key::parse_sequence(&sequence)?,
            repeat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<ChordCommand> {
        ChordCommand::parse(&Key::parse_sequence(input)?)
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
        for (input, command) in [
            ("0s", Some(MetaChordCommand::Settings)),
            ("0r", Some(MetaChordCommand::Reload)),
            ("0q", Some(MetaChordCommand::Quit)),
            ("0h12", Some(MetaChordCommand::Halt(12))),
            ("0", None),
            ("0h", None),
            ("0h12x", None),
            ("00ab", None),
        ] {
            assert_eq!(parse(input).unwrap(), ChordCommand::Meta(command));
        }
        assert!(parse("0h4294967296").is_err());
    }
}
