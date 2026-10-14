use super::Key;
use regex::Regex;
use serde::ser::{Serialize, SerializeStruct, Serializer};

#[derive(Debug, Clone)]
pub enum ChordTrigger {
    Keys(Vec<Key>),
    Pattern(Regex),
}

impl ChordTrigger {
    pub fn matches(&self, keys: &[Key]) -> bool {
        self.matches_in_namespace(keys, false)
    }

    pub(crate) fn matches_in_namespace(&self, keys: &[Key], core: bool) -> bool {
        // Even broad user regexes cannot claim the meta namespace.
        if !core
            && keys
                .first()
                .is_some_and(|key| key.to_sequence_char() == Some('0'))
        {
            return false;
        }
        match self {
            ChordTrigger::Keys(trigger_keys) => trigger_keys == keys,
            ChordTrigger::Pattern(regex) => {
                let Some(sequence_str) = Key::serialize_sequence(keys) else {
                    return false;
                };

                regex.is_match(&sequence_str)
            }
        }
    }
}

impl Serialize for ChordTrigger {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ChordTrigger::Keys(keys) => {
                let mut s = serializer.serialize_struct("ChordTrigger", 1)?;
                s.serialize_field("keys", keys)?;
                s.end()
            }
            ChordTrigger::Pattern(regex) => {
                let mut s = serializer.serialize_struct("ChordTrigger", 1)?;
                s.serialize_field("pattern", regex.as_str())?;
                s.end()
            }
        }
    }
}
