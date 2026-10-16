//! Typed backslash escapes. `\1`–`\9` are numbered buffer references that expand
//! as soon as their digit is typed. Every other escape stays exactly as typed until
//! Tab replaces it with its active expansion; those never expand on their own.

use super::key::SPECIAL_KEY_SEQUENCES;
use super::{ChordMacroName, ChordMacros, Key};
use anyhow::Result;
use keycode::KeyMappingCode;

fn buffer_digit(key: Option<&Key>) -> Option<char> {
    key.and_then(Key::to_sequence_char)
        .filter(|c| ('1'..='9').contains(c))
}

fn is_escape(input: &[Key], index: usize) -> bool {
    input[index].0 == KeyMappingCode::Backslash && buffer_digit(input.get(index + 1)).is_none()
}

/// Whether any escape still waits for Tab.
pub fn has_pending_escape(input: &[Key]) -> bool {
    (0..input.len()).any(|index| is_escape(input, index))
}

/// Replaces the numbered buffer reference that `input` ends with by the buffer's
/// complete expansion, as typing its digit does. Returns `None` when `input` does not
/// end with a reference or the buffer cannot expand, which leaves the reference as
/// typed so running the input reports why.
pub fn expand_typed_buffer(input: &[Key], macros: &ChordMacros) -> Option<Vec<Key>> {
    let [rest @ .., backslash, digit] = input else {
        return None;
    };
    if backslash.0 != KeyMappingCode::Backslash {
        return None;
    }
    let digit = buffer_digit(Some(digit))?;
    let sequence = macros.expand(&format!(r"\{digit}")).ok()??;
    let expanded = Key::parse_sequence(&sequence).ok()?;
    Some([rest, &expanded].concat())
}

/// Expands the numbered buffer references still in the input, such as those in text
/// or one that could not expand when typed. Returns `None` while an escape waits for
/// Tab or a buffer is undefined.
pub fn expand_typed_input(input: &[Key], macros: &ChordMacros) -> Result<Option<Vec<Key>>> {
    let mut output = Vec::with_capacity(input.len());
    let mut keys = input.iter();
    while let Some(&key) = keys.next() {
        if key.0 != KeyMappingCode::Backslash {
            output.push(key);
            continue;
        }
        let Some(digit) = buffer_digit(keys.next()) else {
            return Ok(None);
        };
        let Some(sequence) = macros.expand(&format!(r"\{digit}"))? else {
            return Ok(None);
        };
        let Ok(expanded) = Key::parse_sequence(&sequence) else {
            return Ok(None);
        };
        output.extend(expanded);
    }
    Ok(Some(output))
}

/// References Tab can select, in suggestion order: named macros, then special keys.
fn references(macros: &ChordMacros) -> impl Iterator<Item = (String, Option<ChordMacroName>)> {
    let named = macros
        .entries()
        .into_iter()
        .filter_map(|entry| match entry.target {
            ChordMacroName::Named(name) => {
                Some((format!(r"\={name}="), Some(ChordMacroName::Named(name))))
            }
            ChordMacroName::Buffer(_) => None,
        });
    let keys = SPECIAL_KEY_SEQUENCES
        .iter()
        .map(|(token, _)| ((*token).to_owned(), None));
    named.chain(keys)
}

/// Replaces the escape at the end of `input` with the first expansion whose
/// reference starts with what was typed, as Tab does. A named macro inserts its
/// complete expansion. Returns `None` when nothing matches.
pub fn select_escape(input: &[Key], macros: &ChordMacros) -> Result<Option<Vec<Key>>> {
    let Some(start) = input
        .iter()
        .rposition(|key| key.0 == KeyMappingCode::Backslash)
        .filter(|&start| is_escape(input, start))
    else {
        return Ok(None);
    };
    let Some(typed) = input[start..]
        .iter()
        .map(Key::to_sequence_char)
        .collect::<Option<String>>()
    else {
        return Ok(None);
    };
    let Some((reference, _)) =
        references(macros).find(|(reference, _)| reference.starts_with(&typed))
    else {
        return Ok(None);
    };
    let Some(sequence) = macros.expand(&reference)? else {
        return Ok(None);
    };
    let mut output = input[..start].to_vec();
    output.extend(Key::parse_sequence(&sequence)?);
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(input: &str) -> Vec<Key> {
        // Typing produces each character's key; escapes are never parsed as notation.
        input
            .chars()
            .map(|c| c.to_string().parse().unwrap())
            .collect()
    }

    fn select(input: &str, macros: &ChordMacros) -> Option<String> {
        select_escape(&typed(input), macros)
            .unwrap()
            .map(|keys| Key::serialize_sequence(&keys).unwrap())
    }

    #[test]
    fn typed_escapes_wait_for_tab_while_numbered_buffers_expand() {
        let mut macros = ChordMacros::default();
        macros.set(ChordMacroName::Buffer(1), r"a\up".into());
        macros.set(ChordMacroName::Named("size".into()), "12".into());
        for input in [
            r"a\",
            r"a\u",
            r"a\up",
            r"a\n",
            r"\=size=",
            r"\=size=b",
            r"\1\up",
        ] {
            assert!(has_pending_escape(&typed(input)), "{input}");
            assert_eq!(expand_typed_input(&typed(input), &macros).unwrap(), None);
        }
        assert!(!has_pending_escape(&typed(r"b\1c")));
        assert_eq!(
            expand_typed_input(&typed(r"b\1c"), &macros).unwrap(),
            Some(Key::parse_sequence(r"ba\upc").unwrap())
        );
        // Undefined buffers stay pending.
        assert_eq!(expand_typed_input(&typed(r"\2"), &macros).unwrap(), None);
        // Selected keys are ordinary keys, including Enter.
        let selected = Key::parse_sequence(r"a\up\nb").unwrap();
        assert_eq!(
            expand_typed_input(&selected, &macros).unwrap(),
            Some(selected)
        );
    }

    #[test]
    fn typing_a_buffer_digit_inlines_the_buffer() {
        let mut macros = ChordMacros::default();
        macros.set(ChordMacroName::Buffer(1), r"a\up".into());
        macros.set(ChordMacroName::Buffer(2), r"\1b".into());
        macros.set(ChordMacroName::Buffer(3), r"\3".into());
        let expand = |input: &str| {
            expand_typed_buffer(&typed(input), &macros)
                .map(|keys| Key::serialize_sequence(&keys).unwrap())
        };
        assert_eq!(expand(r"3\1").as_deref(), Some(r"3a\up"));
        assert_eq!(expand(r"\u\2").as_deref(), Some(r"\ua\upb"));
        // Only a reference just typed expands, and only when its buffer can.
        for input in ["", "1", r"\", r"\1x", r"\0", r"\4", r"\3"] {
            assert_eq!(expand(input), None, "{input}");
        }
    }

    #[test]
    fn tab_selects_the_first_matching_expansion() {
        let mut macros = ChordMacros::default();
        for (input, expected) in [
            (r"a\u", r"a\up"),
            (r"a\up", r"a\up"),
            (r"a\l", r"a\left"),
            (r"a\n", r"a\n"),
            (r"\d", r"\down"),
            (r"\", r"\up"),
        ] {
            assert_eq!(select(input, &macros).as_deref(), Some(expected), "{input}");
        }
        assert_eq!(
            select(r"a\n", &macros).map(|s| Key::parse_sequence(&s).unwrap()),
            Some(vec![Key(KeyMappingCode::KeyA), Key(KeyMappingCode::Enter)])
        );
        for input in ["", "ab", r"\x", r"\upx", r"a\1", r"\=size"] {
            assert_eq!(select(input, &macros), None, "{input}");
        }

        // Named macros insert their complete expansion and come before special keys.
        macros.set(ChordMacroName::Buffer(1), "800x600".into());
        macros.set(ChordMacroName::Named("window-size".into()), r"\1".into());
        macros.set(ChordMacroName::Named("wide".into()), "1720x864".into());
        macros.set(ChordMacroName::Named("up-twice".into()), r"\up\up".into());
        for (input, expected) in [
            (r"/ws\=win", "/ws800x600"),
            (r"/ws\=window-size=", "/ws800x600"),
            (r"/ws\=w", "/ws1720x864"),
            (r"\=", r"\up\up"),
            (r"\=u", r"\up\up"),
            (r"\u", r"\up"),
            (r"\n", r"\n"),
            (r"\1\u", r"\1\up"),
        ] {
            assert_eq!(select(input, &macros).as_deref(), Some(expected), "{input}");
        }
        assert_eq!(select(r"\upt", &macros), None);
        // A macro that cannot expand yet is not inserted.
        macros.set(ChordMacroName::Named("broken".into()), r"\=missing=".into());
        assert_eq!(select(r"\=b", &macros), None);
    }
}
