use crate::define_observable;
use serde::Serialize;
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunningChordLoop {
    pub id: u32,
    pub input: String,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetaChordHint {
    pub sequence: String,
    pub name: String,
}

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChordExecutionState {
    pub loops: Vec<RunningChordLoop>,
    pub meta_commands: Vec<MetaChordHint>,
}

impl Default for ChordExecutionState {
    fn default() -> Self {
        Self {
            loops: vec![],
            meta_commands: [
                ("0", "Chord settings"),
                ("0s", "Settings"),
                ("0r", "Reload config"),
                ("0q", "Quit Chord"),
                ("0h", "Halt loop by ID"),
            ]
            .into_iter()
            .map(|(sequence, name)| MetaChordHint {
                sequence: sequence.into(),
                name: name.into(),
            })
            .collect(),
        }
    }
}

define_observable!(
    pub struct ChordExecutionObservable(ChordExecutionState);
    id: "chord-execution";
);
