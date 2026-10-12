use super::chord_sequence::{ChordSequence, ChordSequenceDispatch, ChordTaskQueue};
use crate::app::AppHandleExt;
use crate::app::chord_runner::release_chord_sequences;
use crate::app::state::AppSingleton;
use crate::models::{ChordEventTrigger, ChordInputEvent, ChordKeyboardEvent, Key, KeyEvent};
use crate::state::{ChordInputObservable, ChordInputState, Observable};
use anyhow::{Result, bail};
use keycode::KeyMappingCode;
use nject::injectable;
use parking_lot::Mutex;
use std::collections::HashSet;
use tauri::AppHandle;
use tokio::sync::oneshot;

#[injectable]
pub struct ChordInputManager {
    #[inject(Mutex::new(None))]
    active_task_run: Mutex<Option<(ChordInputEvent, oneshot::Sender<()>)>>,
    /// Repeated keydowns notify handlers without inserting duplicate buffer characters.
    #[inject(Mutex::new(HashSet::new()))]
    held_keys: Mutex<HashSet<Key>>,
    #[inject(Mutex::new(ChordSequence::default()))]
    sequence: Mutex<ChordSequence>,
    #[inject(ChordTaskQueue::default())]
    task_queue: ChordTaskQueue,
    handle: AppHandle,
    observable: ChordInputObservable,
}

impl ChordInputManager {
    pub fn reset(&self) -> Result<()> {
        self.held_keys.lock().clear();
        self.end_active_task();
        self.reset_matching_chords()?;
        self.observable.set_state(|_| ChordInputState::default())
    }

    /// Execute a complete chord sequence without synthesizing keyboard input.
    pub fn execute_sequence(&self, sequence: &str) -> Result<()> {
        let input = Key::parse_sequence(sequence)?;
        if input.len() < 2 {
            bail!("a chord sequence must contain at least two keys");
        }
        self.end_active_task();
        self.reset_matching_chords()?;
        let dispatch = self.resolve_dispatch(&input, &[ChordEventTrigger::Input], None)?;
        let Some(event) = dispatch.input_event.clone() else {
            self.reset_matching_chords()?;
            bail!("no chord is configured for `{sequence}`");
        };
        self.set_executed_event(event)?;
        self.run_dispatch(dispatch, false)?;
        self.reset_matching_chords()
    }

    // Called only while Chord mode is active.
    pub fn handle_key_event(&self, key_event: &KeyEvent) -> Result<()> {
        let fresh_press = should_handle_held_key_event(&mut self.held_keys.lock(), key_event);
        let (KeyEvent::Press(key) | KeyEvent::Release(key)) = key_event;
        let keyboard = self.handle.app_state().keyboard();
        let Some(mut device_keys) = keyboard.state().keys() else {
            return Ok(());
        };
        // Account for the event even if the OS key-state query has not caught up yet.
        match key_event {
            KeyEvent::Press(_) => {
                if !device_keys.contains(key) {
                    device_keys.push(*key);
                }
            }
            KeyEvent::Release(_) => device_keys.retain(|held| held != key),
        }
        let event = ChordKeyboardEvent::new(key_event, &device_keys, !fresh_press);
        let press_triggers = [ChordEventTrigger::Keydown, ChordEventTrigger::Keypress];

        if let KeyEvent::Release(Key(code)) = key_event {
            let should_end = self
                .active_task_run
                .lock()
                .as_ref()
                .is_some_and(|(input, _)| {
                    *code == KeyMappingCode::CapsLock
                        || input.input.last().is_some_and(|key| key.0 == *code)
                });
            if should_end {
                self.end_active_task();
            }
        }

        if !fresh_press {
            return self.dispatch_current_buffer(&press_triggers, &event);
        }
        let is_shift_pressed = event.shift_key;
        if Self::should_toggle_indicator_for_tab_event(key_event, is_shift_pressed) {
            if matches!(key_event, KeyEvent::Press(_)) {
                self.handle
                    .app_state()
                    .chord_mode_manager()
                    .panel
                    .toggle()?;
            }
            return self.dispatch_current_buffer(
                if matches!(key_event, KeyEvent::Press(_)) {
                    &press_triggers
                } else {
                    &[ChordEventTrigger::Keyup]
                },
                &event,
            );
        }
        if Key::modifiers().contains(key)
            || device_keys
                .iter()
                .any(|key| Key::non_shift_modifiers().contains(key))
        {
            return self.dispatch_current_buffer(
                if matches!(key_event, KeyEvent::Press(_)) {
                    &press_triggers
                } else {
                    &[ChordEventTrigger::Keyup]
                },
                &event,
            );
        }

        match key_event {
            KeyEvent::Release(Key(code)) => {
                self.dispatch_current_buffer(&[ChordEventTrigger::Keyup], &event)?;
                let state = self.observable.get_state()?;
                if *code == KeyMappingCode::Space
                    && Self::should_execute_key_buffer_on_release(&state)
                {
                    let dispatch = self.resolve_dispatch(
                        &state.input,
                        &[ChordEventTrigger::Input],
                        Some(&event),
                    )?;
                    if let Some(input_event) = dispatch.input_event.clone() {
                        self.set_executed_event(input_event)?;
                    }
                    self.run_dispatch(dispatch, false)?;
                    self.clear_key_buffer()?;
                }
            }
            KeyEvent::Press(Key(KeyMappingCode::CapsLock)) => {
                let state = self.observable.get_state()?;
                if state.input.is_empty() {
                    self.dispatch_current_buffer(&press_triggers, &event)?;
                    if let Some(selected) = state.selected_input_event {
                        let dispatch = self.resolve_dispatch(
                            &selected.input,
                            &[ChordEventTrigger::Input],
                            Some(&event),
                        )?;
                        self.complete_dispatch(dispatch)?;
                    }
                } else {
                    let dispatch = self.resolve_dispatch(
                        &state.input,
                        &[
                            ChordEventTrigger::Keydown,
                            ChordEventTrigger::Keypress,
                            ChordEventTrigger::Input,
                        ],
                        Some(&event),
                    )?;
                    self.complete_dispatch(dispatch)?;
                }
            }
            KeyEvent::Press(Key(KeyMappingCode::Escape)) => {
                self.clear_key_buffer()?;
                self.dispatch_current_buffer(&press_triggers, &event)?;
            }
            KeyEvent::Press(Key(KeyMappingCode::Space)) => {
                self.dispatch_current_buffer(&press_triggers, &event)?;
            }
            KeyEvent::Press(key) => {
                if is_shift_pressed && Self::should_clear_key_buffer_on_shifted_press(key) {
                    self.clear_key_buffer()?;
                    self.dispatch_current_buffer(&press_triggers, &event)?;
                } else if is_shift_pressed && key.to_sequence_char().is_some() {
                    self.handle_shifted_key_press(key, &event)?;
                } else {
                    self.handle_unshifted_key_press(key, &event)?;
                }
            }
        }
        Ok(())
    }

    fn resolve_dispatch(
        &self,
        keys: &[Key],
        triggers: &[ChordEventTrigger],
        keyboard_event: Option<&ChordKeyboardEvent>,
    ) -> Result<ChordSequenceDispatch> {
        let event = ChordInputEvent {
            input: keys.to_vec(),
            application_id: self.handle.app_state().frontmost().frontmost()?,
        };
        let contexts = self
            .handle
            .app_state()
            .chord_package_manager()
            .matching_chord_contexts(&event);
        self.sequence
            .lock()
            .dispatch(&event, &contexts, triggers, keyboard_event, 1)
    }

    fn dispatch_current_buffer(
        &self,
        triggers: &[ChordEventTrigger],
        event: &ChordKeyboardEvent,
    ) -> Result<()> {
        let state = self.observable.get_state()?;
        self.run_dispatch(
            self.resolve_dispatch(&state.input, triggers, Some(event))?,
            false,
        )
    }

    fn end_active_task(&self) {
        if let Some((_, release)) = self.active_task_run.lock().take() {
            let _ = release.send(());
        }
    }

    fn run_dispatch(&self, dispatch: ChordSequenceDispatch, hold_input: bool) -> Result<()> {
        if dispatch.tasks.is_empty() && dispatch.expired.is_empty() {
            return Ok(());
        }
        let release = if let Some(input_event) = dispatch.input_event.clone().filter(|_| hold_input)
        {
            self.end_active_task();
            let (sender, receiver) = oneshot::channel();
            *self.active_task_run.lock() = Some((input_event, sender));
            Some(receiver)
        } else {
            None
        };
        let handle = self.handle.clone();
        self.task_queue.enqueue(async move {
            let result = async {
                release_chord_sequences(handle.clone(), dispatch.expired).await?;
                let runner = handle.app_state().chord_action_task_runner();
                let mut release = release;
                let count = dispatch.tasks.len();
                for (index, task) in dispatch.tasks.into_iter().enumerate() {
                    let task_run = runner.start_task(&task)?;
                    if index + 1 == count {
                        if let Some(release) = release.take() {
                            let _ = release.await;
                        }
                    }
                    runner.end_task(task_run).await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                log::error!("error running chord events: {error:#}");
            }
        });
        Ok(())
    }

    fn reset_matching_chords(&self) -> Result<()> {
        let expired = self.sequence.lock().reset();
        self.run_dispatch(
            ChordSequenceDispatch {
                expired,
                ..Default::default()
            },
            false,
        )
    }

    fn set_executed_event(&self, event: ChordInputEvent) -> Result<()> {
        self.observable.set_state(move |state| ChordInputState {
            input: vec![],
            pressed_input_event: Some(event.clone()),
            selected_input_event: Some(event),
            ..state
        })
    }

    fn complete_dispatch(&self, dispatch: ChordSequenceDispatch) -> Result<()> {
        if let Some(event) = dispatch.input_event.clone() {
            self.set_executed_event(event)?;
            self.run_dispatch(dispatch, true)?;
            self.reset_matching_chords()
        } else {
            self.run_dispatch(dispatch, false)?;
            self.clear_key_buffer()
        }
    }

    fn should_execute_key_buffer_on_release(state: &ChordInputState) -> bool {
        !state.input.is_empty() && state.selected_input_event.is_none()
    }

    fn should_toggle_indicator_for_tab_event(key_event: &KeyEvent, is_shift_pressed: bool) -> bool {
        matches!(
            key_event,
            KeyEvent::Press(Key(KeyMappingCode::Tab)) | KeyEvent::Release(Key(KeyMappingCode::Tab))
        ) && is_shift_pressed
    }

    fn next_key_buffer_for_unshifted_press(key_buffer: &[Key], key: &Key) -> Vec<Key> {
        let mut next_key_buffer = key_buffer.to_vec();
        if key.0 == KeyMappingCode::Backspace {
            next_key_buffer.pop();
        } else if key.to_sequence_char().is_some() {
            next_key_buffer.push(*key);
        }
        next_key_buffer
    }

    fn handle_unshifted_key_press(&self, key: &Key, event: &ChordKeyboardEvent) -> Result<()> {
        let state = self.observable.get_state()?;
        let next_key_buffer = Self::next_key_buffer_for_unshifted_press(&state.input, key);
        self.observable.set_state(|state| ChordInputState {
            input: next_key_buffer,
            selected_input_event: None,
            pressed_input_event: None,
            ..state
        })?;
        self.dispatch_current_buffer(
            &[ChordEventTrigger::Keydown, ChordEventTrigger::Keypress],
            event,
        )
    }

    fn should_clear_key_buffer_on_shifted_press(key: &Key) -> bool {
        key.0 == KeyMappingCode::Backspace
    }

    fn clear_key_buffer(&self) -> Result<()> {
        self.end_active_task();
        self.observable.set_state(|state| ChordInputState {
            input: vec![],
            selected_input_event: None,
            pressed_input_event: None,
            ..state
        })?;
        self.reset_matching_chords()
    }

    fn handle_shifted_key_press(&self, key: &Key, event: &ChordKeyboardEvent) -> Result<()> {
        let state = self.observable.get_state()?;
        let mut sequence = state.input;
        if sequence.is_empty() {
            let Some(selected) = state.selected_input_event else {
                return self.dispatch_current_buffer(
                    &[ChordEventTrigger::Keydown, ChordEventTrigger::Keypress],
                    event,
                );
            };
            sequence = selected.input;
            sequence.pop();
        }
        sequence.push(*key);
        let dispatch = self.resolve_dispatch(
            &sequence,
            &[
                ChordEventTrigger::Keydown,
                ChordEventTrigger::Keypress,
                ChordEventTrigger::Input,
            ],
            Some(event),
        )?;
        if dispatch.input_event.is_some() {
            self.complete_dispatch(dispatch)
        } else {
            self.observable.set_state(|state| ChordInputState {
                input: sequence,
                selected_input_event: None,
                pressed_input_event: None,
                ..state
            })?;
            self.run_dispatch(dispatch, false)
        }
    }
}

fn should_handle_held_key_event(held_keys: &mut HashSet<Key>, key_event: &KeyEvent) -> bool {
    match key_event {
        KeyEvent::Press(key) => held_keys.insert(*key),
        KeyEvent::Release(key) => {
            held_keys.remove(key);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_edits_the_buffer_and_non_text_keys_preserve_it() {
        let buffer = Key::parse_sequence("j,a").unwrap();
        assert_eq!(
            ChordInputManager::next_key_buffer_for_unshifted_press(
                &buffer,
                &Key(KeyMappingCode::Backspace)
            ),
            Key::parse_sequence("j,").unwrap()
        );
        for key in [
            KeyMappingCode::ArrowLeft,
            KeyMappingCode::Enter,
            KeyMappingCode::ShiftLeft,
        ] {
            assert_eq!(
                ChordInputManager::next_key_buffer_for_unshifted_press(&buffer, &Key(key)),
                buffer
            );
        }
        assert!(
            ChordInputManager::next_key_buffer_for_unshifted_press(
                &[],
                &Key(KeyMappingCode::Backspace)
            )
            .is_empty()
        );
    }

    #[test]
    fn key_repeat_is_observed_without_reinserting_the_character() {
        let mut held = HashSet::new();
        let key = Key(KeyMappingCode::KeyJ);
        assert!(should_handle_held_key_event(
            &mut held,
            &KeyEvent::Press(key)
        ));
        assert!(!should_handle_held_key_event(
            &mut held,
            &KeyEvent::Press(key)
        ));
        assert!(should_handle_held_key_event(
            &mut held,
            &KeyEvent::Release(key)
        ));
        assert!(should_handle_held_key_event(
            &mut held,
            &KeyEvent::Press(key)
        ));
    }
}
