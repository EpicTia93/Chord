use super::caps_listener::CapsHandle;
use super::caps_lock_restore::CapsLockRestore;
use super::health::{Backoff, InputHealth};
use super::secure_input::SecureInputState;
use super::tap::TapHandle;
use crate::app::AppHandleExt;
use crate::models::{AppKeyboardState, Key, KeyEvent, KeyEventAction};
use anyhow::Result;
use bitflags::bitflags;
use keycode::KeyMappingCode;
use nject::injectable;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tauri::AppHandle;

const CAPS_LOCK_QUEUE_CAPACITY: usize = 16;
const KEY_EVENT_QUEUE_CAPACITY: usize = 1_024;

/// How often the supervisor re-checks permissions and listener health.
pub const SUPERVISOR_INTERVAL: Duration = Duration::from_millis(250);
/// With no readable keyboard for this long, the HID manager is recreated:
/// device enumeration can go stale after Karabiner or sleep churn.
pub const CAPS_RESCAN_AFTER: Duration = Duration::from_secs(10);
/// How often the OS Caps Lock toggle is sampled while the key is up, so a
/// toggle made elsewhere (another app, Karabiner) becomes the new baseline.
const CAPS_LOCK_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

bitflags! {
  pub struct Modifiers: u16 {
      const LEFT_SHIFT      = 1 << 0;
      const RIGHT_SHIFT     = 1 << 1;
      const LEFT_CONTROL    = 1 << 2;
      const RIGHT_CONTROL   = 1 << 3;
      const LEFT_OPTION     = 1 << 4;
      const RIGHT_OPTION    = 1 << 5;
      const LEFT_COMMAND    = 1 << 6;
      const RIGHT_COMMAND   = 1 << 7;
      const FUNCTION        = 1 << 8;
  }
}

/// The listener threads currently owned by the supervisor.
#[derive(Default)]
pub(super) struct Listeners {
    tap: Option<TapHandle>,
    tap_backoff: Backoff,
    caps: Option<CapsHandle>,
    caps_backoff: Backoff,
    secure_input: SecureInputState,
}

#[injectable]
pub struct AppKeyboard {
    keyboard_state: AppKeyboardState,

    #[inject(AtomicU16::new(0))]
    pub modifier_flags: AtomicU16,

    #[inject(Arc::new(InputHealth::default()))]
    health: Arc<InputHealth>,

    #[inject(Mutex::new(Listeners::default()))]
    listeners: Mutex<Listeners>,

    #[inject(CapsLockRestore::default())]
    caps_lock_restore: CapsLockRestore,

    handle: AppHandle,
}

impl AppKeyboard {
    pub fn state(&self) -> &AppKeyboardState {
        &self.keyboard_state
    }

    pub fn health(&self) -> &InputHealth {
        &self.health
    }

    /// Both listeners are up and Caps Lock can be observed. Input ownership
    /// (see `DevLockfileDetector`) is only claimed while this holds.
    pub fn input_handlers_running(&self) -> bool {
        self.health.tap_running() && self.health.caps_ready(Instant::now())
    }

    pub fn reset_input_state(&self) {
        self.keyboard_state.reset();
        self.modifier_flags.store(0, Ordering::Relaxed);
        self.caps_lock_restore.reset();
    }

    /// Chord mode was entered with Caps+Space: put the OS Caps Lock toggle
    /// back to what it was before the Caps press. Called by the controller.
    pub fn on_chord_mode_activated(&self) {
        if !self.keyboard_state.is_caps_pressed() {
            return;
        }
        let before = self.caps_lock_restore.chord_activated();
        self.restore_caps_lock(before);
    }

    fn restore_caps_lock(&self, enabled: bool) {
        log::debug!("Restoring Caps Lock toggle to {enabled} after Chord activation");
        if let Err(error) = Self::set_caps_lock(enabled) {
            log::warn!("Failed to restore the Caps Lock toggle: {error:#}");
        }
    }

    /// Reads the OS toggle while Caps is up. Skipped during a hold, when the
    /// OS may be mid-toggle.
    fn sample_os_caps_lock(&self) {
        if self.keyboard_state.is_caps_pressed() {
            return;
        }
        match Self::get_caps_lock() {
            Ok(enabled) => self.caps_lock_restore.observe(enabled),
            Err(error) => log::debug!("Failed to read the Caps Lock toggle: {error:#}"),
        }
    }

    /// Starts the supervisor thread that owns listener registration for the
    /// lifetime of the process. Permissions are polled here as well, so a
    /// grant made in System Settings is picked up without the settings UI.
    pub(super) fn start_supervisor(&self) -> Result<()> {
        let handle = self.handle.clone();
        std::thread::Builder::new()
            .name("chord-input-supervisor".into())
            .spawn(move || {
                let mut permissions_error_reported = false;
                let mut last_caps_lock_sample: Option<Instant> = None;
                loop {
                    let state = handle.app_state();
                    if last_caps_lock_sample
                        .is_none_or(|last| last.elapsed() >= CAPS_LOCK_SAMPLE_INTERVAL)
                    {
                        last_caps_lock_sample = Some(Instant::now());
                        state.keyboard().sample_os_caps_lock();
                    }
                    let permissions = match state.permissions().refresh_if_changed() {
                        Ok(permissions) => {
                            permissions_error_reported = false;
                            permissions
                        }
                        Err(error) => {
                            if !permissions_error_reported {
                                permissions_error_reported = true;
                                log::error!("Failed to refresh input permissions: {error:#}");
                            }
                            (false, false)
                        }
                    };
                    state.keyboard().reconcile_listeners(permissions, Instant::now());
                    state.keyboard().check_secure_input();
                    std::thread::sleep(SUPERVISOR_INTERVAL);
                }
            })?;
        Ok(())
    }

    fn listeners(&self) -> MutexGuard<'_, Listeners> {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Brings the listeners in line with the granted permissions, restarting
    /// anything that died. `(accessibility, input_monitoring)`.
    pub(super) fn reconcile_listeners(&self, (accessibility, input_monitoring): (bool, bool), now: Instant) {
        let mut listeners = self.listeners();
        self.reconcile_tap(&mut listeners, accessibility, now);
        self.reconcile_caps(&mut listeners, input_monitoring, now);
    }

    fn reconcile_tap(&self, listeners: &mut Listeners, accessibility: bool, now: Instant) {
        if listeners.tap.as_ref().is_some_and(TapHandle::is_finished) {
            log::error!("Keyboard event tap thread exited unexpectedly; scheduling a restart");
            if let Some(tap) = listeners.tap.take() {
                tap.stop();
            }
            listeners.tap_backoff.failed(now);
        }
        if !accessibility {
            if let Some(tap) = listeners.tap.take() {
                log::warn!("Accessibility permission was revoked; stopping the keyboard event tap");
                tap.stop();
            }
            return;
        }
        if listeners.tap.is_some() || !listeners.tap_backoff.ready(now) {
            return;
        }
        match self.spawn_tap() {
            Ok(tap) => {
                if listeners.tap_backoff.failures() > 0 {
                    log::info!(
                        "Keyboard event tap recovered after {} failed attempt(s)",
                        listeners.tap_backoff.failures()
                    );
                }
                listeners.tap_backoff.succeeded();
                listeners.tap = Some(tap);
            }
            Err(error) => {
                let delay = listeners.tap_backoff.failed(now);
                log::error!(
                    "Failed to start the keyboard event tap (retrying in {delay:?}): {error:#}"
                );
            }
        }
    }

    fn reconcile_caps(&self, listeners: &mut Listeners, input_monitoring: bool, now: Instant) {
        if listeners.caps.as_ref().is_some_and(CapsHandle::is_finished) {
            log::error!("Caps Lock listener thread exited unexpectedly; scheduling a restart");
            if let Some(caps) = listeners.caps.take() {
                caps.stop();
            }
            listeners.caps_backoff.failed(now);
        }
        if !input_monitoring {
            if let Some(caps) = listeners.caps.take() {
                log::warn!("Input Monitoring permission was revoked; stopping the Caps Lock listener");
                caps.stop();
            }
            return;
        }
        if listeners.caps.is_some() && self.caps_needs_rescan(now) {
            log::warn!(
                "No readable keyboard for {CAPS_RESCAN_AFTER:?}; recreating the Caps Lock HID manager"
            );
            if let Some(caps) = listeners.caps.take() {
                caps.stop();
            }
        }
        if listeners.caps.is_some() || !listeners.caps_backoff.ready(now) {
            return;
        }
        match self.spawn_caps_listener() {
            Ok(caps) => {
                if listeners.caps_backoff.failures() > 0 {
                    log::info!(
                        "Caps Lock listener recovered after {} failed attempt(s)",
                        listeners.caps_backoff.failures()
                    );
                }
                listeners.caps_backoff.succeeded();
                listeners.caps = Some(caps);
            }
            Err(error) => {
                let delay = listeners.caps_backoff.failed(now);
                log::error!(
                    "Failed to start the Caps Lock listener (retrying in {delay:?}): {error:#}"
                );
            }
        }
    }

    fn caps_needs_rescan(&self, now: Instant) -> bool {
        self.health.caps_open_devices() == 0
            && self
                .health
                .caps_devices_lost_at()
                .is_some_and(|lost_at| now.duration_since(lost_at) >= CAPS_RESCAN_AFTER)
    }

    fn check_secure_input(&self) {
        let current = SecureInputState::current();
        let mut listeners = self.listeners();
        if listeners.secure_input == current {
            return;
        }
        match &current {
            SecureInputState::Disabled => {
                log::warn!("Secure Input is disabled again; keyboard events reach Chord")
            }
            enabled => log::warn!(
                "Secure Input is {}; macOS withholds keyboard events from Chord until it is disabled",
                enabled.describe()
            ),
        }
        listeners.secure_input = current;
    }

    /// Creates the tap and the thread that runs the controller for its events.
    fn spawn_tap(&self) -> Result<TapHandle> {
        let (tx, rx) = sync_channel::<(u64, KeyEvent)>(KEY_EVENT_QUEUE_CAPACITY);
        let handle = self.handle.clone();
        // Keeping the tap callback minimal: controller work runs here instead.
        std::thread::Builder::new()
            .name("chord-key-events".into())
            .spawn(move || {
                while let Ok((generation, event)) = rx.recv() {
                    handle
                        .app_state()
                        .dev_lockfile_detector()
                        .with_input_generation(generation, || {
                            if let Err(error) =
                                handle.app_state().app_controller().handle_key_event(&event)
                            {
                                log::error!("Failed to handle key event: {error:#}");
                            }
                        });
                }
            })?;

        let handle = self.handle.clone();
        let mut queue_full_reported = false;
        let mut disconnected_reported = false;
        let sink = move |event: KeyEvent| -> KeyEventAction {
            let keyboard = handle.app_state().keyboard();
            handle
                .app_state()
                .dev_lockfile_detector()
                .with_input_owner(|generation| {
                    let action = keyboard.handle_key_event(&event);
                    match tx.try_send((generation, event)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            if !queue_full_reported {
                                queue_full_reported = true;
                                log::warn!(
                                    "Dropping keyboard events because the bounded input queue is saturated"
                                );
                            }
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            if !disconnected_reported {
                                disconnected_reported = true;
                                log::error!("Keyboard event handler thread is gone");
                            }
                        }
                    }
                    action
                })
                .unwrap_or(KeyEventAction::Forward)
        };
        TapHandle::spawn(self.health.clone(), Box::new(sink))
    }

    /// Creates the Caps Lock listener and the thread that applies its events.
    fn spawn_caps_listener(&self) -> Result<CapsHandle> {
        let (tx, rx) = sync_channel::<bool>(CAPS_LOCK_QUEUE_CAPACITY);
        let handle = self.handle.clone();
        std::thread::Builder::new()
            .name("chord-caps-events".into())
            .spawn(move || {
                while let Ok(pressed) = rx.recv() {
                    let key = Key(KeyMappingCode::CapsLock);
                    let event = if pressed {
                        KeyEvent::Press(key)
                    } else {
                        KeyEvent::Release(key)
                    };
                    // Waits out an ownership handoff instead of dropping the event.
                    handle
                        .app_state()
                        .dev_lockfile_detector()
                        .with_current_input_owner(|_generation| {
                            handle.app_state().keyboard().handle_key_event(&event);
                            if let Err(error) =
                                handle.app_state().app_controller().handle_key_event(&event)
                            {
                                log::error!("Failed to handle Caps Lock event: {error:#}");
                            }
                        });
                }
            })?;

        let handle = self.handle.clone();
        let mut queue_full_reported = false;
        let mut disconnected_reported = false;
        let sink = move |pressed: bool| {
            // Mirror the physical key immediately so a Space arriving on the
            // event tap moments later sees the right state.
            handle.app_state().keyboard().state().set_caps_pressed(pressed);
            match tx.try_send(pressed) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    if !queue_full_reported {
                        queue_full_reported = true;
                        log::warn!(
                            "Dropping Caps Lock events because the bounded input queue is saturated"
                        );
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    if !disconnected_reported {
                        disconnected_reported = true;
                        log::error!("Caps Lock event handler thread is gone");
                    }
                }
            }
        };
        CapsHandle::spawn(self.health.clone(), Box::new(sink))
    }

    pub fn emit_caps_lock() -> Result<()> {
        let rc = unsafe { toggle_caps() };
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("failed to toggle caps lock state via native layer: {rc}")
        }
    }

    pub fn set_caps_lock_off() -> Result<()> {
        Self::set_caps_lock(false)
    }

    pub fn set_caps_lock(enabled: bool) -> Result<()> {
        let rc = unsafe {
            if enabled {
                set_caps_on()
            } else {
                set_caps_off()
            }
        };
        if rc == 0 {
            Ok(())
        } else {
            anyhow::bail!("failed to set caps lock state via native layer: {rc}")
        }
    }

    pub fn get_caps_lock() -> Result<bool> {
        let mut state: c_int = 0;
        let rc = unsafe { get_caps_state(&mut state) };
        if rc == 0 {
            Ok(state != 0)
        } else {
            anyhow::bail!("failed to read caps lock state via native layer: {rc}")
        }
    }

    /// Updates the tracked keyboard state and decides whether the event
    /// reaches other applications. Runs on the listener threads, so it must
    /// stay cheap.
    fn handle_key_event(&self, event: &KeyEvent) -> KeyEventAction {
        log::debug!("Processing event {event:?}");
        self.keyboard_state.handle_key_event(event);
        self.update_modifier_flags(event);

        let app_mode = self.handle.app_state().app_controller().app_mode();

        // A plain Caps press toggles Caps Lock as usual. Only a press that
        // activates Chord mode (see `on_chord_mode_activated`) restores the
        // toggle; the release restores it once more because the OS toggle
        // may land after the activation on keyboards with a Caps Lock delay.
        match event {
            KeyEvent::Press(Key(KeyMappingCode::CapsLock)) => self.caps_lock_restore.pressed(),
            KeyEvent::Release(Key(KeyMappingCode::CapsLock)) => {
                if let Some(before) = self.caps_lock_restore.released() {
                    self.restore_caps_lock(before);
                }
                // Re-sample the OS toggle now that the key is up; a plain press just toggled it.
                self.sample_os_caps_lock();
            }
            _ => {}
        }

        // Space while Caps is held enters Chord mode and is never typed.
        if event == &KeyEvent::Press(Key(KeyMappingCode::Space))
            && self.keyboard_state.is_caps_pressed()
        {
            return KeyEventAction::Consume;
        }

        if app_mode.is_chord() {
            return KeyEventAction::Consume;
        }

        KeyEventAction::Forward
    }

    #[allow(dead_code)]
    pub fn get_modifier_flags(&self) -> Modifiers {
        Modifiers::from_bits_truncate(self.modifier_flags.load(Ordering::Relaxed))
    }

    fn modifier_key_to_flag(key: &Key) -> Option<Modifiers> {
        let flag = match key.0 {
            KeyMappingCode::ShiftLeft => Modifiers::LEFT_SHIFT,
            KeyMappingCode::ShiftRight => Modifiers::RIGHT_SHIFT,
            KeyMappingCode::ControlLeft => Modifiers::LEFT_CONTROL,
            KeyMappingCode::ControlRight => Modifiers::RIGHT_CONTROL,
            KeyMappingCode::AltLeft => Modifiers::LEFT_OPTION,
            KeyMappingCode::AltRight => Modifiers::RIGHT_OPTION,
            KeyMappingCode::MetaLeft => Modifiers::LEFT_COMMAND,
            KeyMappingCode::MetaRight => Modifiers::RIGHT_COMMAND,
            KeyMappingCode::Fn => Modifiers::FUNCTION,
            _ => return None,
        };

        Some(flag)
    }

    fn update_modifier_flags(&self, event: &KeyEvent) {
        match event {
            KeyEvent::Press(key) => {
                if let Some(flag) = Self::modifier_key_to_flag(key) {
                    self.modifier_flags.fetch_or(flag.bits(), Ordering::Relaxed);
                }
            }
            KeyEvent::Release(key) => {
                if let Some(flag) = Self::modifier_key_to_flag(key) {
                    self.modifier_flags
                        .fetch_and(!flag.bits(), Ordering::Relaxed);
                }
            }
        }
    }
}

unsafe extern "C" {
    fn toggle_caps() -> c_int;
    fn set_caps_on() -> c_int;
    fn set_caps_off() -> c_int;
    fn get_caps_state(out_state: *mut c_int) -> c_int;
}
