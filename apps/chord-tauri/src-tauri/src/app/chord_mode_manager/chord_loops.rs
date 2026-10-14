use crate::app::AppHandleExt;
use crate::app::chord_runner::ChordActionTask;
use crate::models::{ChordTaskAction, Key};
use crate::state::{ChordExecutionObservable, MetaChordHint, Observable, RunningChordLoop};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::watch;

#[derive(Default)]
struct LoopRegistry {
    next_id: u32,
    active: BTreeMap<u32, (String, watch::Sender<bool>)>,
}

impl LoopRegistry {
    fn reserve(&mut self, input: String) -> Result<(u32, watch::Receiver<bool>)> {
        self.next_id = self.next_id.checked_add(1).context("loop IDs exhausted")?;
        let (cancel, receiver) = watch::channel(false);
        self.active.insert(self.next_id, (input, cancel));
        Ok((self.next_id, receiver))
    }

    fn remove(&mut self, id: u32) -> bool {
        if let Some((_, cancel)) = self.active.remove(&id) {
            let _ = cancel.send(true);
            true
        } else {
            false
        }
    }

    fn snapshot(&self) -> Vec<RunningChordLoop> {
        self.active
            .iter()
            .map(|(&id, (input, _))| RunningChordLoop {
                id,
                input: input.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Default)]
pub(super) struct ChordLoops {
    registry: Arc<Mutex<LoopRegistry>>,
}

impl ChordLoops {
    pub fn configure_meta_commands(
        &self,
        observable: &ChordExecutionObservable,
        hints: Vec<MetaChordHint>,
        halt_loop_template: String,
    ) -> Result<()> {
        let registry = self.registry.lock();
        observable.set_state(|mut state| {
            state.loops = registry.snapshot();
            state.meta_commands = hints;
            state.halt_loop_template = halt_loop_template;
            state
        })
    }

    pub fn reserve(
        &self,
        task: &ChordActionTask,
        observable: &ChordExecutionObservable,
    ) -> Result<(u32, watch::Receiver<bool>)> {
        let input = Key::serialize_sequence(&task.event.input).context("invalid loop chord")?;
        let mut registry = self.registry.lock();
        let halt_template = observable.get_state()?.halt_loop_template;
        let (id, receiver) = registry.reserve(input)?;
        if let Err(error) = observable.set_state(|mut state| {
            state.loops = registry.snapshot();
            state
        }) {
            registry.remove(id);
            return Err(error);
        }
        let halt = halt_template.replace("{id}", &id.to_string());
        log::info!("Started chord loop {id}; halt with {halt}");
        Ok((id, receiver))
    }

    pub fn halt(&self, id: u32, observable: &ChordExecutionObservable) -> Result<()> {
        let mut registry = self.registry.lock();
        anyhow::ensure!(registry.remove(id), "no chord loop is running with ID {id}");
        observable.set_state(|mut state| {
            state.loops = registry.snapshot();
            state
        })
    }

    pub fn stop_all(&self, observable: &ChordExecutionObservable) -> Result<()> {
        let mut registry = self.registry.lock();
        for (_, cancel) in registry.active.values() {
            let _ = cancel.send(true);
        }
        registry.active.clear();
        observable.set_state(|mut state| {
            state.loops = vec![];
            state
        })
    }

    pub fn finish(&self, id: u32, observable: &ChordExecutionObservable) {
        let mut registry = self.registry.lock();
        registry.remove(id);
        if let Err(error) = observable.set_state(|mut state| {
            state.loops = registry.snapshot();
            state
        }) {
            log::error!("failed to publish chord loop completion: {error:#}");
        }
    }
}

/// Loop one whole action at a time; each iteration completes before another starts.
/// The short pause also gives input and other chords time to run for synchronous actions.
async fn repeat_until_halted<F, Fut>(
    mut cancelled: watch::Receiver<bool>,
    mut iteration: F,
) -> Result<()>
where
    F: FnMut(watch::Receiver<bool>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    while !*cancelled.borrow() {
        iteration(cancelled.clone()).await?;
        tokio::select! {
            biased;
            _ = cancelled.changed() => break,
            _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
    }
    Ok(())
}

pub(super) async fn run_loop(
    handle: AppHandle,
    task: ChordActionTask,
    cancelled: watch::Receiver<bool>,
) -> Result<()> {
    let result = repeat_until_halted(cancelled, |mut cancelled| {
        let handle = handle.clone();
        let task = task.clone();
        async move {
            if *cancelled.borrow() {
                return Ok(());
            }
            let runner = handle.app_state().chord_action_task_runner();
            let mut run = runner.start_task(&task)?;
            tokio::select! {
                biased;
                _ = cancelled.changed() => {
                    if matches!(run, crate::app::chord_runner::ChordActionTaskRun::Handler(_)) {
                        // A worker is shared by its package's handlers. Let this one
                        // invocation finish instead of killing other loops in the VM.
                        runner.end_task(&mut run).await
                    } else {
                        runner.abort_task(run)
                    }
                },
                result = runner.end_task(&mut run) => result,
            }
        }
    })
    .await;
    if let ChordTaskAction::Handler(action) = task.action {
        if let Some(id) = action.sequence_id {
            tauri::async_runtime::spawn(crate::bun_js::package_worker::release_sequences(vec![id]));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn loops_survive_later_chords_and_halt_independently() {
        let mut registry = LoopRegistry::default();
        let (id, receiver) = registry.reserve("ab".into()).unwrap();
        let (other_id, other_receiver) = registry.reserve("cd".into()).unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let loop_calls = calls.clone();
        let (started, ready) = oneshot::channel();
        let mut started = Some(started);
        let run = tokio::spawn(repeat_until_halted(receiver, move |_| {
            let calls = loop_calls.clone();
            let started = started.take();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                if let Some(started) = started {
                    let _ = started.send(());
                }
                Ok(())
            }
        }));
        ready.await.unwrap();
        // Later work runs while the loop is still alive.
        let later = tokio::spawn(async { "later chord" });
        assert_eq!(later.await.unwrap(), "later chord");
        assert!(!run.is_finished());
        assert!(registry.remove(id));
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let stopped_at = calls.load(Ordering::SeqCst);
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), stopped_at);
        assert!(!*other_receiver.borrow());
        assert_eq!(
            registry.snapshot(),
            [RunningChordLoop {
                id: other_id,
                input: "cd".into()
            }]
        );
        assert!(!registry.remove(id));
        let (next_id, _) = registry.reserve("ef".into()).unwrap();
        assert!(next_id > other_id);
    }

    #[tokio::test]
    async fn halted_pending_loops_never_start_and_failures_end_repetition() {
        let mut registry = LoopRegistry::default();
        let (id, receiver) = registry.reserve("ab".into()).unwrap();
        registry.remove(id);
        repeat_until_halted(receiver, |_| async { panic!("halted loop ran") })
            .await
            .unwrap();
        let (_, receiver) = registry.reserve("cd".into()).unwrap();
        let mut calls = 0;
        let error = repeat_until_halted(receiver, |_| {
            calls += 1;
            async { anyhow::bail!("handler failed") }
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.to_string(), "handler failed");
    }
}
