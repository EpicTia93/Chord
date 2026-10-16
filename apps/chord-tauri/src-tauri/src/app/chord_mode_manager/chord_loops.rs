use crate::app::AppHandleExt;
use crate::app::chord_runner::ChordActionTask;
use crate::models::{ChordRepeat, ChordTaskAction};
use crate::state::{ChordExecutionObservable, MetaChordHint, Observable, RunningChordLoop};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::watch;

/// Unconfigured intervals give input and other chords time to run for synchronous actions.
const DEFAULT_INTERVAL: Duration = Duration::from_millis(10);

/// Chords that run in the background, one whole step at a time, until they finish
/// or are halted.
#[derive(Debug, Clone)]
pub(super) struct ChordPlan {
    /// The command as typed, shown next to its halt command.
    pub input: String,
    /// Never empty.
    pub steps: Vec<ChordActionTask>,
    pub repeat: ChordRepeat,
    /// Pause before the first run only.
    pub delay: Duration,
    /// Pauses after each run, used in order and cycled.
    pub intervals: Vec<Duration>,
    /// Handler scopes owned by this plan, released when it ends.
    pub scopes: Vec<String>,
}

impl ChordPlan {
    fn total_runs(&self) -> Option<u64> {
        match self.repeat {
            ChordRepeat::Times(count) => Some(u64::from(count) * self.steps.len() as u64),
            ChordRepeat::Loop => None,
        }
    }

    fn interval_after(&self, run: u64) -> Duration {
        if self.intervals.is_empty() {
            DEFAULT_INTERVAL
        } else {
            self.intervals[(run % self.intervals.len() as u64) as usize]
        }
    }
}

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
        input: String,
        observable: &ChordExecutionObservable,
    ) -> Result<(u32, watch::Receiver<bool>)> {
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

/// Waits out `duration`, or returns false once a halt lands first.
async fn pause(cancelled: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    if duration.is_zero() {
        // Run the next chord right away, but stay cooperative so a halt can land.
        tokio::task::yield_now().await;
        return true;
    }
    tokio::select! {
        biased;
        _ = cancelled.changed() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

/// After the delay, run one whole step at a time; each completes before the next starts.
async fn run_steps<F, Fut>(
    plan: &ChordPlan,
    mut cancelled: watch::Receiver<bool>,
    mut step: F,
) -> Result<()>
where
    F: FnMut(&ChordActionTask, watch::Receiver<bool>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let total = plan.total_runs();
    if !plan.delay.is_zero() && !pause(&mut cancelled, plan.delay).await {
        return Ok(());
    }
    let mut run = 0;
    while !*cancelled.borrow() && total.is_none_or(|total| run < total) {
        let index = (run % plan.steps.len() as u64) as usize;
        step(&plan.steps[index], cancelled.clone()).await?;
        let interval = plan.interval_after(run);
        run += 1;
        if total.is_some_and(|total| run >= total) || !pause(&mut cancelled, interval).await {
            break;
        }
    }
    Ok(())
}

/// Handlers of a completed input act on the window that was focused when its run
/// started. `read` runs once, before any callback, and every handler task and plan step
/// receives its result, however often the run repeats or wherever focus moves meanwhile.
pub(super) async fn select_run_window<F, Fut>(
    tasks: &mut [ChordActionTask],
    plan: Option<&mut ChordPlan>,
    read: F,
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Option<u32>>,
{
    let steps = plan
        .map(|plan| plan.steps.as_mut_slice())
        .unwrap_or_default();
    let actions: Vec<_> = tasks
        .iter_mut()
        .chain(steps)
        .filter_map(|task| match &mut task.action {
            ChordTaskAction::Handler(action) => Some(action),
            _ => None,
        })
        .collect();
    if actions.is_empty() {
        return;
    }
    let window = read().await;
    for action in actions {
        action.focused_window_id = window;
    }
}

pub(super) async fn run_plan(
    handle: AppHandle,
    plan: ChordPlan,
    cancelled: watch::Receiver<bool>,
) -> Result<()> {
    let result = run_steps(&plan, cancelled, |task, mut cancelled| {
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
    if !plan.scopes.is_empty() {
        tauri::async_runtime::spawn(crate::bun_js::package_worker::release_sequences(
            plan.scopes,
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ChordInputEvent, HandlerChordAction, ShellChordAction};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::oneshot;

    fn plan(steps: &[&str], repeat: ChordRepeat, intervals: &[u64]) -> ChordPlan {
        ChordPlan {
            input: steps.join(r"\n"),
            steps: steps
                .iter()
                .map(|command| ChordActionTask {
                    package_name: "@fixture/test".into(),
                    initiator_file_pathslug: "chords/macos.toml".into(),
                    event: ChordInputEvent::default(),
                    action: ChordTaskAction::Shell(ShellChordAction {
                        command: (*command).into(),
                    }),
                    num_times: 1,
                })
                .collect(),
            repeat,
            delay: Duration::ZERO,
            intervals: intervals
                .iter()
                .copied()
                .map(Duration::from_millis)
                .collect(),
            scopes: vec![],
        }
    }

    fn command(task: &ChordActionTask) -> String {
        match &task.action {
            ChordTaskAction::Shell(action) => action.command.clone(),
            _ => panic!("expected fixture step"),
        }
    }

    fn handler_task(input: &str) -> ChordActionTask {
        ChordActionTask {
            action: ChordTaskAction::Handler(HandlerChordAction {
                handler_id: "window".into(),
                event_args: vec!["SPACE".into(), "NEXT".into()],
                input: input.into(),
                sequence_id: None,
                assign_to: None,
                keyboard_event: None,
                focused_window_id: None,
            }),
            ..plan(&[input], ChordRepeat::Times(1), &[]).steps.remove(0)
        }
    }

    fn windows(tasks: &[ChordActionTask]) -> Vec<Option<u32>> {
        tasks
            .iter()
            .filter_map(|task| match &task.action {
                ChordTaskAction::Handler(action) => Some(action.focused_window_id),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn runs_select_their_window_once_for_every_handler_step() {
        // A run's callbacks, and every step and iteration of its plan, share one read.
        let mut tasks = vec![handler_task("/ws]")];
        let mut looped = plan(&["/w]"], ChordRepeat::Times(3), &[0]);
        looped.steps.insert(0, handler_task("/ws]"));
        let mut reads = 0;
        select_run_window(&mut tasks, Some(&mut looped), || {
            reads += 1;
            async { Some(42) }
        })
        .await;
        assert_eq!(reads, 1);
        assert_eq!(windows(&tasks), [Some(42)]);
        assert_eq!(windows(&looped.steps), [Some(42)]);
        assert!(matches!(looped.steps[1].action, ChordTaskAction::Shell(_)));

        // A run without handlers never queries the focused application.
        let mut shortcuts = plan(&["/w]"], ChordRepeat::Loop, &[]);
        select_run_window(&mut [], Some(&mut shortcuts), || async {
            panic!("runs without handlers must not read the focused window")
        })
        .await;
    }

    #[tokio::test]
    async fn loops_survive_later_chords_and_halt_independently() {
        let mut registry = LoopRegistry::default();
        let (id, receiver) = registry.reserve("ab".into()).unwrap();
        let (other_id, other_receiver) = registry.reserve("cd".into()).unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let loop_calls = calls.clone();
        let (started, ready) = oneshot::channel();
        let mut started = Some(started);
        let looped = plan(&["ab"], ChordRepeat::Loop, &[]);
        let run = tokio::spawn(async move {
            run_steps(&looped, receiver, move |_, _| {
                let calls = loop_calls.clone();
                let started = started.take();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(started) = started {
                        let _ = started.send(());
                    }
                    Ok(())
                }
            })
            .await
        });
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
        run_steps(
            &plan(&["ab"], ChordRepeat::Loop, &[]),
            receiver,
            |_, _| async { panic!("halted loop ran") },
        )
        .await
        .unwrap();
        let (_, receiver) = registry.reserve("cd".into()).unwrap();
        let mut calls = 0;
        let error = run_steps(&plan(&["cd"], ChordRepeat::Loop, &[]), receiver, |_, _| {
            calls += 1;
            async { anyhow::bail!("handler failed") }
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.to_string(), "handler failed");
    }

    #[tokio::test]
    async fn finite_sequences_run_every_chord_in_order_then_finish() {
        let (_cancel, receiver) = watch::channel(false);
        let mut calls = Vec::new();
        run_steps(
            &plan(&["ab", "cd"], ChordRepeat::Times(3), &[0]),
            receiver,
            |task, _| {
                calls.push(command(task));
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, ["ab", "cd", "ab", "cd", "ab", "cd"]);
    }

    #[tokio::test]
    async fn configured_intervals_cycle_between_runs_and_zero_runs_immediately() {
        let sequence = plan(&["ab", "cd"], ChordRepeat::Times(2), &[1, 3]);
        let intervals: Vec<_> = (0..4).map(|run| sequence.interval_after(run)).collect();
        assert_eq!(intervals, [1, 3, 1, 3].map(Duration::from_millis));
        let single = plan(&["ab"], ChordRepeat::Times(3), &[1, 3]);
        assert_eq!(single.interval_after(2), Duration::from_millis(1));
        assert_eq!(single.total_runs(), Some(3));
        assert_eq!(plan(&["ab"], ChordRepeat::Loop, &[]).total_runs(), None);
        assert_eq!(
            plan(&["ab"], ChordRepeat::Loop, &[]).interval_after(7),
            DEFAULT_INTERVAL
        );

        // Zero intervals never wait for a timer tick.
        let (_cancel, receiver) = watch::channel(false);
        let started = std::time::Instant::now();
        let mut calls = 0;
        run_steps(
            &plan(&["ab", "cd"], ChordRepeat::Times(50), &[0]),
            receiver,
            |_, _| {
                calls += 1;
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls, 100);
        assert!(started.elapsed() < Duration::from_millis(100));

        // Intervals are observed and a halt during one ends the sequence.
        let (cancel, receiver) = watch::channel(false);
        let calls = Arc::new(AtomicU32::new(0));
        let run_calls = calls.clone();
        let slow = plan(&["ab"], ChordRepeat::Times(3), &[900]);
        let run = tokio::spawn(async move {
            run_steps(&slow, receiver, move |_, _| {
                run_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cancel.send(true).unwrap();
        tokio::time::timeout(Duration::from_millis(200), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_delay_precedes_only_the_first_run_and_a_halt_during_it_runs_nothing() {
        let delay = Duration::from_millis(150);
        let mut delayed = plan(&["ab", "cd"], ChordRepeat::Times(2), &[0]);
        delayed.delay = delay;
        let (_cancel, receiver) = watch::channel(false);
        let started = std::time::Instant::now();
        let mut calls = Vec::new();
        run_steps(&delayed, receiver, |task, _| {
            calls.push((command(task), started.elapsed()));
            async { Ok(()) }
        })
        .await
        .unwrap();
        let commands: Vec<_> = calls.iter().map(|(command, _)| command.as_str()).collect();
        assert_eq!(commands, ["ab", "cd", "ab", "cd"]);
        assert!(calls[0].1 >= delay);
        // Later runs and steps follow their intervals, without waiting again.
        assert!(calls[3].1 - calls[0].1 < delay);

        let (cancel, receiver) = watch::channel(false);
        let mut halted = plan(&["ab"], ChordRepeat::Loop, &[]);
        halted.delay = Duration::from_secs(60);
        let run = tokio::spawn(async move {
            run_steps(&halted, receiver, |_, _| async {
                panic!("a run halted during its delay must not start")
            })
            .await
        });
        tokio::task::yield_now().await;
        cancel.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
