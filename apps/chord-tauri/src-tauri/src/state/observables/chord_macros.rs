use crate::define_observable;
use crate::models::ChordMacroEntry;
use serde::Serialize;
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChordMacrosState {
    pub entries: Vec<ChordMacroEntry>,
}

define_observable!(
    pub struct ChordMacrosObservable(ChordMacrosState);
    id: "chord-macros";
);
