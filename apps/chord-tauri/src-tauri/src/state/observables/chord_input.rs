use crate::define_observable;
use crate::models::{ChordInput, ChordInputEvent};
use serde::Serialize;
use typeshare::typeshare;

#[typeshare]
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChordInputState {
    /// The keys a user has pressed for a chord but not yet executed. Think of it as the equivalent
    /// of an HTML <input />'s `value`. Always cleared when executed.
    pub input: ChordInput,

    /// Insert-mode text for each quote in `input`, in order; see `ChordInputEvent::quotes`.
    pub quotes: Vec<String>,

    /// `input` ends with an open quote, so keys are typed into its text instead of
    /// the chord. Chord stays active without Space until Caps+Space closes the quote.
    pub inserting: bool,

    /// The chord currently being pressed down.
    pub pressed_input_event: Option<ChordInputEvent>,

    /// The chord that is "active"
    pub selected_input_event: Option<ChordInputEvent>,

    /// Resolved action name for the current buffer or the last executed chord.
    pub description: Option<String>,

    /// Why the selected input failed to execute. Failed input stays selected, so it is
    /// shown like an executed chord until the next key press.
    pub error: Option<String>,
}

define_observable!(
    pub struct ChordInputObservable(ChordInputState);
    id: "chord-input";
);
