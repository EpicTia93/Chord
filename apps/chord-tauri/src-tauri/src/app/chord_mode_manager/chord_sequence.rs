use crate::app::chord_package_manager::{ChordPackage, ChordReference};
use crate::app::chord_runner::ChordActionTask;
use crate::models::{
    ChordAction, ChordCommand, ChordEventTrigger, ChordInputEvent, ChordKeyboardEvent, ChordRepeat,
    ChordTaskAction, MetaChordCommand,
};
use anyhow::Result;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use tauri::async_runtime::JoinHandle;
use tokio::sync::oneshot;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ChordIdentity {
    package: String,
    file: PathBuf,
    index: u32,
    pattern: String,
    application: Option<String>,
}

impl ChordIdentity {
    fn new(reference: &ChordReference, event: &ChordInputEvent) -> Self {
        Self {
            package: reference.package_name.clone(),
            file: reference.chords_file_pathslug.clone(),
            index: reference.chord.index,
            pattern: reference.chord.raw_trigger.clone(),
            application: event.application_id.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct ChordSequence {
    active: HashMap<ChordIdentity, String>,
}

#[derive(Default)]
pub(super) struct ChordSequenceDispatch {
    pub tasks: Vec<ChordActionTask>,
    pub expired: Vec<String>,
    pub input_event: Option<ChordInputEvent>,
    pub meta_command: Option<MetaChordCommand>,
    pub loop_task: Option<ChordActionTask>,
}

impl ChordSequence {
    pub fn dispatch_command(
        &mut self,
        raw_event: ChordInputEvent,
        triggers: &[ChordEventTrigger],
        keyboard_event: Option<&ChordKeyboardEvent>,
        matching: impl FnOnce(&ChordInputEvent) -> Vec<(ChordPackage, ChordReference)>,
    ) -> Result<ChordSequenceDispatch> {
        let (keys, repeat) = match ChordCommand::parse(&raw_event.input)? {
            ChordCommand::Meta(command) => {
                let command = command.filter(|_| triggers.contains(&ChordEventTrigger::Input));
                return Ok(ChordSequenceDispatch {
                    expired: self.reset(),
                    input_event: command.map(|_| raw_event),
                    meta_command: command,
                    ..Default::default()
                });
            }
            ChordCommand::Pending => {
                return Ok(ChordSequenceDispatch {
                    expired: self.reset(),
                    ..Default::default()
                });
            }
            ChordCommand::Run { keys, repeat } => (keys, repeat),
        };
        let event = ChordInputEvent {
            input: keys,
            application_id: raw_event.application_id.clone(),
        };
        let contexts = matching(&event);
        let num_times = match repeat {
            ChordRepeat::Times(count) => count,
            ChordRepeat::Loop => 1,
        };
        let mut dispatch = self.dispatch(&event, &contexts, triggers, keyboard_event, num_times)?;
        if dispatch.input_event.is_some() {
            dispatch.input_event = Some(raw_event);
            if repeat == ChordRepeat::Loop {
                if let Some(task) = dispatch.tasks.pop() {
                    if let ChordTaskAction::Handler(action) = &task.action {
                        if let Some(id) = &action.sequence_id {
                            self.detach(id);
                        }
                    }
                    dispatch.loop_task = Some(task);
                }
            }
        }
        Ok(dispatch)
    }

    pub fn reset(&mut self) -> Vec<String> {
        self.active.drain().map(|(_, id)| id).collect()
    }

    /// A background loop owns its persistent values until it halts or fails.
    pub fn detach(&mut self, id: &str) {
        self.active.retain(|_, active_id| active_id != id);
    }

    pub fn dispatch(
        &mut self,
        event: &ChordInputEvent,
        contexts: &[(ChordPackage, ChordReference)],
        triggers: &[ChordEventTrigger],
        keyboard_event: Option<&ChordKeyboardEvent>,
        num_times: u32,
    ) -> Result<ChordSequenceDispatch> {
        let matching: HashSet<_> = contexts
            .iter()
            .filter(|(_, r)| {
                r.chord
                    .actions
                    .iter()
                    .any(|a| matches!(a, ChordAction::Emit(_)))
            })
            .map(|(_, r)| ChordIdentity::new(r, event))
            .collect();
        let mut dispatch = ChordSequenceDispatch::default();
        self.active.retain(|identity, id| {
            if matching.contains(identity) {
                true
            } else {
                dispatch.expired.push(id.clone());
                false
            }
        });
        // All initializers precede the keyboard/input callbacks for this buffer snapshot.
        for (package, reference) in contexts {
            if !reference
                .chord
                .actions
                .iter()
                .any(|a| matches!(a, ChordAction::Emit(_)))
            {
                continue;
            }
            let identity = ChordIdentity::new(reference, event);
            if !self.active.contains_key(&identity) {
                let id = uuid::Uuid::new_v4().to_string();
                self.active.insert(identity, id.clone());
                Self::append_tasks(
                    &mut dispatch.tasks,
                    package,
                    reference,
                    event,
                    ChordEventTrigger::Init,
                    &id,
                    keyboard_event,
                    1,
                )?;
            }
        }
        for &trigger in triggers {
            for (package, reference) in contexts {
                let id = self.active.get(&ChordIdentity::new(reference, event));
                if trigger == ChordEventTrigger::Input {
                    if let Some(action) = reference
                        .chord
                        .actions
                        .iter()
                        .find(|a| a.trigger() == trigger)
                    {
                        if let Some(task) = package.resolve_action_task(
                            event.clone(),
                            reference,
                            action,
                            num_times,
                            id.cloned(),
                            keyboard_event.cloned(),
                        )? {
                            dispatch.input_event = Some(event.clone());
                            dispatch.tasks.push(task);
                        }
                        // Ordinary chord completion retains the existing first-match priority.
                        break;
                    }
                } else if let Some(id) = id {
                    Self::append_tasks(
                        &mut dispatch.tasks,
                        package,
                        reference,
                        event,
                        trigger,
                        id,
                        keyboard_event,
                        1,
                    )?;
                }
            }
        }
        Ok(dispatch)
    }

    fn append_tasks(
        tasks: &mut Vec<ChordActionTask>,
        package: &ChordPackage,
        reference: &ChordReference,
        event: &ChordInputEvent,
        trigger: ChordEventTrigger,
        id: &str,
        keyboard_event: Option<&ChordKeyboardEvent>,
        num_times: u32,
    ) -> Result<()> {
        let keyboard_event = keyboard_event.cloned().map(|mut event| {
            if trigger == ChordEventTrigger::Keypress {
                event.r#type = "keypress".into();
            }
            event
        });
        for action in reference
            .chord
            .actions
            .iter()
            .filter(|a| a.trigger() == trigger)
        {
            if let Some(task) = package.resolve_action_task(
                event.clone(),
                reference,
                action,
                num_times,
                Some(id.to_string()),
                keyboard_event.clone(),
            )? {
                tasks.push(task);
            }
        }
        Ok(())
    }
}

/// Reserve callback order on the keyboard thread, before spawning any async work.
#[derive(Default)]
pub(super) struct ChordTaskQueue {
    tail: Mutex<Option<oneshot::Receiver<()>>>,
}

impl ChordTaskQueue {
    pub fn enqueue(&self, job: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
        let (completed, next) = oneshot::channel();
        let previous = self.tail.lock().replace(next);
        tauri::async_runtime::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            job.await;
            let _ = completed.send(());
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ChordTaskAction, CompiledChordsFile, CompiledChordsFileHandler, Key, KeyEvent,
        ParsedChordsFile,
    };
    use keycode::KeyMappingCode;
    use std::sync::Arc;

    fn fixture(source: &str) -> ChordPackage {
        let parsed: ParsedChordsFile = source.parse().unwrap();
        let path = PathBuf::from("chords/dev/fixture/macos.toml");
        let handlers = ["prepare", "down", "press", "up", "input", "instagram"]
            .map(|event| CompiledChordsFileHandler {
                event: event.into(),
                handler_id: event.into(),
            })
            .to_vec();
        let file = CompiledChordsFile {
            name: parsed.name,
            pathslug: path.clone(),
            meta: parsed.meta,
            handlers,
            chords: parsed.chords,
            chord_hints: parsed.chord_hints,
        };
        ChordPackage {
            name: "@fixture/test".into(),
            js_package: None,
            raw_chords_files: HashMap::new(),
            compiled_chords_files: HashMap::from([(path, file)]),
            global_chords: vec![],
        }
    }

    fn event(input: &str) -> ChordInputEvent {
        ChordInputEvent {
            input: Key::parse_sequence(input).unwrap(),
            application_id: Some("dev.fixture".into()),
        }
    }

    fn contexts(
        package: &ChordPackage,
        event: &ChordInputEvent,
    ) -> Vec<(ChordPackage, ChordReference)> {
        package
            .matching_app_chords(event)
            .into_iter()
            .map(|r| (package.clone(), r))
            .collect()
    }

    fn handlers(dispatch: &ChordSequenceDispatch) -> Vec<&str> {
        dispatch
            .tasks
            .iter()
            .map(|task| match &task.action {
                ChordTaskAction::Handler(action) => action.handler_id.as_str(),
                _ => panic!("expected handler"),
            })
            .collect()
    }

    #[test]
    fn story_input_tasks_preserve_the_sequence_and_resolve_capture_arguments() {
        let package = fixture(
            r#"
name = "Instagram"
[chords]
'hs.(\d+)' = { '@input:instagram' = ['story', '$1'] }
'hsr(\d+)' = { name = "Home: Stories > Reply @ #", '@input:instagram' = ['story', '$1'] }
"#,
        );
        for input in ["hs.12", "hsr12"] {
            let input_event = event(input);
            let reference = package
                .resolve_chord_from_input_event(&input_event)
                .unwrap();
            let task = package
                .resolve_task(input_event, reference, 1)
                .unwrap()
                .unwrap();
            let ChordTaskAction::Handler(action) = task.action else {
                panic!("expected Instagram handler");
            };
            assert_eq!(action.handler_id, "instagram");
            assert_eq!(action.input, input);
            assert_eq!(action.event_args, vec!["story".into(), "12".into()]);
        }
        for input in ["hs,12", "xhsr12", "hsr12x", "hsr"] {
            assert!(
                package
                    .resolve_chord_from_input_event(&event(input))
                    .is_none()
            );
        }
    }

    #[test]
    fn init_runs_once_until_mismatch_then_restarts_with_a_new_scope() {
        let package = fixture(
            r#"
name = "fixture"
[chords]
'j,(.*)' = { '@init:prepare=state' = [], '@keydown:down=last' = ['$state', '$1'], '@keypress:press' = ['$last'], '@keyup:up' = ['$state'], '@input:input' = ['$state'] }
"#,
        );
        let mut sequence = ChordSequence::default();
        let keyboard_event =
            ChordKeyboardEvent::new(&KeyEvent::Press(Key(KeyMappingCode::Comma)), &[], false);
        let e = event("j,");
        let first = sequence
            .dispatch(
                &e,
                &contexts(&package, &e),
                &[ChordEventTrigger::Keydown, ChordEventTrigger::Keypress],
                Some(&keyboard_event),
                1,
            )
            .unwrap();
        assert_eq!(handlers(&first), ["prepare", "down", "press"]);
        let ChordTaskAction::Handler(init) = &first.tasks[0].action else {
            panic!()
        };
        assert_eq!(init.assign_to.as_deref(), Some("state"));
        let id = init.sequence_id.clone();
        let ChordTaskAction::Handler(down) = &first.tasks[1].action else {
            panic!()
        };
        assert_eq!(down.assign_to.as_deref(), Some("last"));
        assert_eq!(down.event_args[1].as_str(), Some(""));
        let ChordTaskAction::Handler(press) = &first.tasks[2].action else {
            panic!()
        };
        assert_eq!(press.keyboard_event.as_ref().unwrap().r#type, "keypress");

        for (input, trigger, expected) in [
            ("j,a", ChordEventTrigger::Keydown, "down"),
            ("j,a", ChordEventTrigger::Keyup, "up"),
            ("j,", ChordEventTrigger::Keydown, "down"),
            ("j,", ChordEventTrigger::Input, "input"),
        ] {
            let e = event(input);
            let next = sequence
                .dispatch(&e, &contexts(&package, &e), &[trigger], None, 1)
                .unwrap();
            assert_eq!(handlers(&next), [expected]);
            let ChordTaskAction::Handler(action) = &next.tasks[0].action else {
                panic!()
            };
            assert_eq!(action.sequence_id, id);
        }
        let mismatch = event("j");
        let exited = sequence
            .dispatch(
                &mismatch,
                &contexts(&package, &mismatch),
                &[ChordEventTrigger::Keydown],
                None,
                1,
            )
            .unwrap();
        assert!(exited.tasks.is_empty());
        assert_eq!(exited.expired, [id.clone().unwrap()]);
        let reentered = sequence
            .dispatch(
                &e,
                &contexts(&package, &e),
                &[ChordEventTrigger::Keydown],
                None,
                1,
            )
            .unwrap();
        assert_eq!(handlers(&reentered), ["prepare", "down"]);
        let ChordTaskAction::Handler(init) = &reentered.tasks[0].action else {
            panic!()
        };
        assert_ne!(init.sequence_id, id);
        assert_eq!(sequence.reset(), [init.sequence_id.clone().unwrap()]);
    }

    #[test]
    fn overlapping_lifecycle_chords_do_not_shadow_input_and_initialize_independently() {
        let package = fixture(
            r#"
name = "fixture"
[chords]
'j,' = { '@init:prepare' = [], '@keydown:down' = [] }
'j,(.*)' = { '@init:prepare' = [], '@keydown:down' = [], '@input:input' = ['$1'] }
"#,
        );
        let e = event("j,");
        let references = contexts(&package, &e);
        let resolved = package.resolve_chord_from_input_event(&e).unwrap();
        assert_eq!(resolved.chord.raw_trigger, "j,(.*)");
        let mut sequence = ChordSequence::default();
        let dispatch = sequence
            .dispatch(
                &e,
                &references,
                &[ChordEventTrigger::Keydown, ChordEventTrigger::Input],
                None,
                1,
            )
            .unwrap();
        assert_eq!(
            handlers(&dispatch),
            ["prepare", "prepare", "down", "down", "input"]
        );
        let ChordTaskAction::Handler(first) = &dispatch.tasks[0].action else {
            panic!()
        };
        let ChordTaskAction::Handler(second) = &dispatch.tasks[1].action else {
            panic!()
        };
        assert_ne!(first.sequence_id, second.sequence_id);
        assert!(dispatch.input_event.is_some());
    }

    #[test]
    fn shortcut_only_chords_do_not_create_javascript_scopes() {
        let package = fixture("name = 'fixture'\n[chords]\nab = { shortcut = 'cmd+a' }");
        let e = event("ab");
        let mut sequence = ChordSequence::default();
        let dispatch = sequence
            .dispatch(
                &e,
                &contexts(&package, &e),
                &[ChordEventTrigger::Keydown, ChordEventTrigger::Input],
                None,
                1,
            )
            .unwrap();
        assert_eq!(dispatch.tasks.len(), 1);
        assert!(matches!(
            dispatch.tasks[0].action,
            ChordTaskAction::Shortcut(_)
        ));
        assert!(sequence.reset().is_empty());
    }

    #[test]
    fn repeated_input_keeps_capture_arguments_and_initializes_only_once() {
        let package = fixture(
            r#"name = 'fixture'
[chords]
'hsr(\d+)' = { '@init:prepare=state' = [], '@input:input' = ['$state', '$1'] }
"#,
        );
        let mut sequence = ChordSequence::default();
        let raw = event("12hsr34");
        let dispatch = sequence
            .dispatch_command(raw.clone(), &[ChordEventTrigger::Input], None, |e| {
                contexts(&package, e)
            })
            .unwrap();
        assert_eq!(handlers(&dispatch), ["prepare", "input"]);
        assert_eq!(dispatch.tasks[0].num_times, 1);
        assert_eq!(dispatch.tasks[1].num_times, 12);
        assert_eq!(dispatch.input_event, Some(raw));
        let ChordTaskAction::Handler(action) = &dispatch.tasks[1].action else {
            panic!()
        };
        assert_eq!(action.input, "hsr34");
        assert_eq!(action.event_args, ["$state".into(), "34".into()]);
    }

    #[test]
    fn loop_scopes_survive_buffer_reset_and_meta_commands_bypass_packages() {
        let package = fixture(
            "name = 'fixture'\n[chords]\nab = { '@init:prepare=state' = [], '@input:input' = ['$state'] }",
        );
        let mut sequence = ChordSequence::default();
        let dispatch = sequence
            .dispatch_command(event("1ab"), &[ChordEventTrigger::Input], None, |e| {
                contexts(&package, e)
            })
            .unwrap();
        assert_eq!(handlers(&dispatch), ["prepare"]);
        let task = dispatch.loop_task.unwrap();
        assert_eq!(task.num_times, 1);
        let ChordTaskAction::Handler(action) = &task.action else {
            panic!()
        };
        assert!(action.sequence_id.is_some());
        assert!(sequence.reset().is_empty());
        let later = sequence
            .dispatch_command(event("ab"), &[ChordEventTrigger::Input], None, |e| {
                contexts(&package, e)
            })
            .unwrap();
        let ChordTaskAction::Handler(later_action) = &later.tasks[0].action else {
            panic!()
        };
        assert_ne!(later_action.sequence_id, action.sequence_id);
        for input in ["0s", "0r", "0q", "0h12", "0x", "0h12x"] {
            let dispatch = sequence
                .dispatch_command(event(input), &[ChordEventTrigger::Input], None, |_| {
                    panic!("meta commands must not visit packages")
                })
                .unwrap();
            assert!(dispatch.tasks.is_empty() && dispatch.loop_task.is_none());
            assert_eq!(
                dispatch.input_event.is_some(),
                !["0x", "0h12x"].contains(&input)
            );
        }
        let typing = sequence
            .dispatch_command(event("0s"), &[ChordEventTrigger::Keydown], None, |_| {
                panic!("meta commands must not visit packages")
            })
            .unwrap();
        assert!(typing.meta_command.is_none());
    }

    #[tokio::test]
    async fn async_callbacks_cannot_overtake_init_or_earlier_events() {
        let queue = ChordTaskQueue::default();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (started, ready) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        let first_calls = calls.clone();
        let first = queue.enqueue(async move {
            first_calls.lock().push("init-start");
            started.send(()).unwrap();
            gate.await.unwrap();
            first_calls.lock().push("init-end");
        });
        let second_calls = calls.clone();
        let second = queue.enqueue(async move {
            second_calls.lock().push("keydown");
        });
        ready.await.unwrap();
        assert_eq!(*calls.lock(), ["init-start"]);
        release.send(()).unwrap();
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(*calls.lock(), ["init-start", "init-end", "keydown"]);
    }
}
