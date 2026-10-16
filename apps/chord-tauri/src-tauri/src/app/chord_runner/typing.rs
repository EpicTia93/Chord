use crate::app::chord_runner::{ChordActionTask, ShortcutChordActionTaskRunner};
use crate::models::{Key, SimulatedShortcutAction, TypeChordAction};
use anyhow::{Context, Result};
use nject::injectable;
use std::time::Duration;
use tauri::async_runtime::JoinHandle;

#[injectable]
pub struct TypeChordActionTaskRunner {
    shortcut: ShortcutChordActionTaskRunner,
}

#[derive(Debug)]
pub struct TypeChordActionTaskRun {
    join_handle: JoinHandle<Result<()>>,
}

impl TypeChordActionTaskRunner {
    pub fn start(
        &self,
        task: &ChordActionTask,
        action: &TypeChordAction,
    ) -> Result<TypeChordActionTaskRun> {
        let shortcut = self.shortcut.clone();
        let intervals = action
            .intervals_ms
            .iter()
            .map(|&interval| Duration::from_millis(interval.into()))
            .collect();
        let keys = action
            .text
            .chars()
            .map(|character| {
                Key::for_text_char(character)
                    .with_context(|| format!("cannot type {character:?} with a US keyboard layout"))
            })
            .collect::<Result<Vec<_>>>()?;
        let join_handle = tauri::async_runtime::spawn(type_keys(
            task.num_times,
            keys,
            intervals,
            move |(key, shift)| shortcut.simulate_shortcut_actions(key_strokes(key, shift)),
        ));
        Ok(TypeChordActionTaskRun { join_handle })
    }

    pub async fn end(&self, task_run: &mut TypeChordActionTaskRun) -> Result<()> {
        (&mut task_run.join_handle).await?
    }

    pub fn abort(&self, task_run: TypeChordActionTaskRun) -> Result<()> {
        task_run.join_handle.abort();
        Ok(())
    }
}

/// One character's events. Unshifted keys suppress a Shift the user is holding,
/// so text is typed exactly as recorded.
fn key_strokes(key: Key, shift: bool) -> Vec<SimulatedShortcutAction> {
    let shift_key = Key(keycode::KeyMappingCode::ShiftLeft);
    if shift {
        vec![
            SimulatedShortcutAction::Press(shift_key, false),
            SimulatedShortcutAction::Press(key, false),
            SimulatedShortcutAction::Release(key, false),
            SimulatedShortcutAction::Release(shift_key, false),
        ]
    } else {
        vec![
            SimulatedShortcutAction::Press(key, true),
            SimulatedShortcutAction::Release(key, true),
        ]
    }
}

/// Each character is pressed and released before the next, so stopping between
/// them never leaves a key held. Pauses are used in order and cycled; zero is immediate.
async fn type_keys<T: Copy>(
    count: u32,
    keys: Vec<T>,
    intervals: Vec<Duration>,
    mut press: impl FnMut(T) -> Result<()>,
) -> Result<()> {
    let total = u64::from(count) * keys.len() as u64;
    for index in 0..total {
        press(keys[(index % keys.len() as u64) as usize])?;
        if index + 1 == total {
            break;
        }
        match intervals.get((index % intervals.len().max(1) as u64) as usize) {
            Some(pause) if !pause.is_zero() => tokio::time::sleep(*pause).await,
            _ => tokio::task::yield_now().await,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn keys_are_typed_in_order_with_cycled_pauses_between_them() {
        let keys = Key::parse_sequence(r"ab\n").unwrap();
        let mut typed = Vec::new();
        let mut times = Vec::new();
        let started = Instant::now();
        type_keys(
            2,
            keys.clone(),
            [30, 0].map(Duration::from_millis).to_vec(),
            |key| {
                typed.push(key);
                times.push(started.elapsed());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(typed, [keys.clone(), keys].concat());
        // Gaps alternate 30 ms, 0 ms, ... and nothing follows the last key.
        assert!(times[1] - times[0] >= Duration::from_millis(30));
        assert!(times[2] - times[1] < Duration::from_millis(30));
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn untimed_typing_is_immediate_and_failures_stop_it() {
        let started = Instant::now();
        let mut calls = 0;
        type_keys(100, Key::parse_sequence("ab").unwrap(), vec![], |_| {
            calls += 1;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(calls, 200);
        assert!(started.elapsed() < Duration::from_millis(100));
        let mut calls = 0;
        let error = type_keys(1, Key::parse_sequence("abc").unwrap(), vec![], |_| {
            calls += 1;
            anyhow::bail!("simulation failed")
        })
        .await
        .unwrap_err();
        assert_eq!(
            (calls, error.to_string().as_str()),
            (1, "simulation failed")
        );
    }

    #[test]
    fn shifted_characters_hold_shift_and_unshifted_ones_suppress_it() {
        let events = |character| {
            let (key, shift) = Key::for_text_char(character).unwrap();
            key_strokes(key, shift)
                .into_iter()
                .map(|action| match action {
                    SimulatedShortcutAction::Press(key, suppress) => (true, key, suppress),
                    SimulatedShortcutAction::Release(key, suppress) => (false, key, suppress),
                })
                .collect::<Vec<_>>()
        };
        let shift = Key(keycode::KeyMappingCode::ShiftLeft);
        let h = Key(keycode::KeyMappingCode::KeyH);
        assert_eq!(
            events('H'),
            [
                (true, shift, false),
                (true, h, false),
                (false, h, false),
                (false, shift, false)
            ]
        );
        assert_eq!(events('h'), [(true, h, true), (false, h, true)]);
    }
}
