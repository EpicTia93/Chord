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
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChordExecutionState {
    pub loops: Vec<RunningChordLoop>,
    pub meta_commands: Vec<MetaChordHint>,
    pub halt_loop_template: String,
}

define_observable!(
    pub struct ChordExecutionObservable(ChordExecutionState);
    id: "chord-execution";
);
