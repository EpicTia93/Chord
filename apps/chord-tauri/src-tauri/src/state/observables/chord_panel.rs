use crate::define_observable;
use crate::models::ChordLayout;
use serde::Serialize;
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChordPanelState {
    pub is_visible: bool,
    pub layout: ChordLayout,
}

define_observable!(
    pub struct ChordPanelObservable(ChordPanelState);
    id: "chord-panel";
);
