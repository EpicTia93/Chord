//! The keyboard event tap.
//!
//! Chord owns its `CGEventTap` directly instead of going through a library so
//! that the tap can be re-enabled, health-checked, stopped and restarted. A
//! tap that macOS disables (callback timeout, user-input safety cutoff, wake
//! from sleep) is otherwise silently dead: events keep flowing to other apps
//! and nothing tells the process.

use super::health::InputHealth;
use crate::models::{Key, KeyEvent, KeyEventAction};
use anyhow::{Context, Result, anyhow};
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopRunResult, kCFRunLoopCommonModes,
    kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType,
};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Marker rdev puts on the events Chord itself posts; those must pass through untouched.
const SYNTHETIC_EVENT_USER_DATA: [i64; 2] = [0xDEAD_BEEF, 0xDEAD_DEAD];

/// How often the tap thread wakes up to verify the tap is still enabled.
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const START_TIMEOUT: Duration = Duration::from_secs(5);

// Device-dependent modifier bits carried in `CGEventFlags` (`IOLLEvent.h`).
const NX_DEVICELCTLKEYMASK: u64 = 0x0000_0001;
const NX_DEVICELSHIFTKEYMASK: u64 = 0x0000_0002;
const NX_DEVICERSHIFTKEYMASK: u64 = 0x0000_0004;
const NX_DEVICELCMDKEYMASK: u64 = 0x0000_0008;
const NX_DEVICERCMDKEYMASK: u64 = 0x0000_0010;
const NX_DEVICELALTKEYMASK: u64 = 0x0000_0020;
const NX_DEVICERALTKEYMASK: u64 = 0x0000_0040;
const NX_DEVICERCTLKEYMASK: u64 = 0x0000_2000;
// Device-independent modifier bits.
const NX_SHIFTMASK: u64 = 0x0002_0000;
const NX_CONTROLMASK: u64 = 0x0004_0000;
const NX_ALTERNATEMASK: u64 = 0x0008_0000;
const NX_COMMANDMASK: u64 = 0x0010_0000;
const NX_SECONDARYFNMASK: u64 = 0x0080_0000;

pub type TapSink = Box<dyn FnMut(KeyEvent) -> KeyEventAction + Send>;

/// State handed to the C callback. Lives on the tap thread for as long as the
/// tap exists; the callback only runs on that thread.
struct TapContext {
    tap: Option<CFRetained<CFMachPort>>,
    sink: TapSink,
    health: Arc<InputHealth>,
    disabled_notifications: u32,
}

impl TapContext {
    fn handle(&mut self, kind: CGEventType, event: NonNull<CGEvent>) -> KeyEventAction {
        if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput
        {
            self.disabled_notifications += 1;
            if let Some(tap) = &self.tap {
                CGEvent::tap_enable(tap, true);
            }
            self.health.set_tap_enabled(true);
            let reason = if kind == CGEventType::TapDisabledByTimeout {
                "the callback took too long"
            } else {
                "macOS user-input safety cutoff"
            };
            log::warn!(
                "Keyboard event tap was disabled by macOS ({reason}); re-enabled it (occurrence #{})",
                self.disabled_notifications
            );
            return KeyEventAction::Forward;
        }

        // SAFETY: Core Graphics hands the callback a live event for the duration of the call.
        let event = unsafe { event.as_ref() };
        let user_data = CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData);
        if SYNTHETIC_EVENT_USER_DATA.contains(&user_data) {
            return KeyEventAction::Forward;
        }

        let keycode = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode);
        let key_event = match kind {
            CGEventType::KeyDown => key_from_keycode(keycode).map(KeyEvent::Press),
            CGEventType::KeyUp => key_from_keycode(keycode).map(KeyEvent::Release),
            CGEventType::FlagsChanged => {
                decode_flags_changed(keycode, CGEvent::flags(Some(event)).bits())
            }
            _ => None,
        };
        match key_event {
            Some(key_event) => (self.sink)(key_event),
            None => KeyEventAction::Forward,
        }
    }
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user_info` is the `TapContext` boxed by the owning thread, which
    // outlives the tap and never touches the context while the run loop runs.
    let Some(context) = (unsafe { user_info.cast::<TapContext>().as_mut() }) else {
        return event.as_ptr();
    };
    // A panic must never unwind into Core Graphics: that would abort the process.
    match catch_unwind(AssertUnwindSafe(|| context.handle(kind, event))) {
        Ok(KeyEventAction::Consume) => std::ptr::null_mut(),
        Ok(KeyEventAction::Forward) => event.as_ptr(),
        Err(_) => {
            log::error!("Keyboard event tap callback panicked; forwarding the event");
            event.as_ptr()
        }
    }
}

fn key_from_keycode(keycode: i64) -> Option<Key> {
    let keycode = u16::try_from(keycode).ok()?;
    Key::try_from(mac_keycode::Key::from_keycode(keycode)?).ok()
}

/// Turns a modifier `flagsChanged` event into a press or release.
///
/// Caps Lock (keycode 57) is deliberately not decoded here: macOS reports only
/// its toggle, after a hold delay, so it comes from the HID listener instead.
pub(super) fn decode_flags_changed(keycode: i64, flags: u64) -> Option<KeyEvent> {
    use mac_keycode::Key as K;
    let key = K::from_keycode(u16::try_from(keycode).ok()?)?;
    let (own_bit, sibling_bit, generic_bit) = match key {
        K::Shift => (NX_DEVICELSHIFTKEYMASK, NX_DEVICERSHIFTKEYMASK, NX_SHIFTMASK),
        K::RightShift => (NX_DEVICERSHIFTKEYMASK, NX_DEVICELSHIFTKEYMASK, NX_SHIFTMASK),
        K::Control => (NX_DEVICELCTLKEYMASK, NX_DEVICERCTLKEYMASK, NX_CONTROLMASK),
        K::RightControl => (NX_DEVICERCTLKEYMASK, NX_DEVICELCTLKEYMASK, NX_CONTROLMASK),
        K::Option => (NX_DEVICELALTKEYMASK, NX_DEVICERALTKEYMASK, NX_ALTERNATEMASK),
        K::RightOption => (NX_DEVICERALTKEYMASK, NX_DEVICELALTKEYMASK, NX_ALTERNATEMASK),
        K::Command => (NX_DEVICELCMDKEYMASK, NX_DEVICERCMDKEYMASK, NX_COMMANDMASK),
        K::RightCommand => (NX_DEVICERCMDKEYMASK, NX_DEVICELCMDKEYMASK, NX_COMMANDMASK),
        K::Function => (NX_SECONDARYFNMASK, 0, NX_SECONDARYFNMASK),
        _ => return None,
    };
    // Virtual keyboards may omit the device-dependent bits; fall back to the
    // generic modifier bit only when neither side is reported.
    let pressed = if flags & (own_bit | sibling_bit) != 0 {
        flags & own_bit != 0
    } else {
        flags & generic_bit != 0
    };
    let key = Key::try_from(key).ok()?;
    Some(if pressed {
        KeyEvent::Press(key)
    } else {
        KeyEvent::Release(key)
    })
}

/// A running event tap thread. Dropping the handle does not stop the tap;
/// call [`TapHandle::stop`].
pub struct TapHandle {
    thread: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl TapHandle {
    /// Creates the tap on a dedicated thread and returns once it is enabled.
    pub fn spawn(health: Arc<InputHealth>, sink: TapSink) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(1);
        let thread = std::thread::Builder::new()
            .name("chord-key-tap".into())
            .spawn({
                let stop = stop.clone();
                move || run_tap(health, sink, stop, ready_tx)
            })
            .context("spawn keyboard event tap thread")?;
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self { thread, stop }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(anyhow!("keyboard event tap did not start within {START_TIMEOUT:?}"))
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.join();
    }
}

fn run_tap(
    health: Arc<InputHealth>,
    sink: TapSink,
    stop: Arc<AtomicBool>,
    ready_tx: std::sync::mpsc::SyncSender<Result<()>>,
) {
    let mut context = Box::new(TapContext {
        tap: None,
        sink,
        health: health.clone(),
        disabled_notifications: 0,
    });
    let events_of_interest = (1u64 << CGEventType::KeyDown.0)
        | (1u64 << CGEventType::KeyUp.0)
        | (1u64 << CGEventType::FlagsChanged.0);
    // SAFETY: the callback is a valid `CGEventTapCallBack` and the context
    // pointer stays valid until the tap is invalidated below.
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            events_of_interest,
            Some(tap_callback),
            (&mut *context as *mut TapContext).cast(),
        )
    };
    let Some(tap) = tap else {
        let _ = ready_tx.send(Err(anyhow!(
            "CGEventTapCreate failed: Accessibility (or Input Monitoring) is not granted to this executable"
        )));
        return;
    };
    context.tap = Some(tap.clone());
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
        tap.invalidate();
        let _ = ready_tx.send(Err(anyhow!("CFMachPortCreateRunLoopSource failed")));
        return;
    };
    let Some(run_loop) = CFRunLoop::current() else {
        tap.invalidate();
        let _ = ready_tx.send(Err(anyhow!("no run loop on the event tap thread")));
        return;
    };
    // SAFETY: reading a Core Foundation constant.
    let (common_modes, default_mode) = unsafe { (kCFRunLoopCommonModes, kCFRunLoopDefaultMode) };
    run_loop.add_source(Some(&source), common_modes);
    CGEvent::tap_enable(&tap, true);
    health.set_tap_running(true);
    health.set_tap_enabled(true);
    log::info!("Keyboard event tap started");
    let _ = ready_tx.send(Ok(()));

    let started = Instant::now();
    let mut reenabled_by_watchdog = 0u32;
    let exit_reason = loop {
        if stop.load(Ordering::Acquire) {
            break None;
        }
        let result = CFRunLoop::run_in_mode(
            default_mode,
            HEALTH_CHECK_INTERVAL.as_secs_f64(),
            false,
        );
        if result == CFRunLoopRunResult::Finished {
            break Some("the run loop has no sources left".to_string());
        }
        // Belt and braces: the disable notification is not always delivered
        // (notably around sleep/wake), so verify the tap state directly.
        let enabled = CGEvent::tap_is_enabled(&tap);
        if !enabled {
            CGEvent::tap_enable(&tap, true);
            reenabled_by_watchdog += 1;
            log::warn!(
                "Keyboard event tap was found disabled without a notification; re-enabled it (occurrence #{reenabled_by_watchdog})"
            );
        }
        health.set_tap_enabled(CGEvent::tap_is_enabled(&tap));
    };

    CGEvent::tap_enable(&tap, false);
    run_loop.remove_source(Some(&source), common_modes);
    tap.invalidate();
    context.tap = None;
    health.set_tap_running(false);
    match exit_reason {
        None => log::info!(
            "Keyboard event tap stopped on request after {:?}",
            started.elapsed()
        ),
        Some(reason) => log::error!(
            "Keyboard event tap stopped after {:?}: {reason}",
            started.elapsed()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keycode::KeyMappingCode;

    const SHIFT: i64 = 0x38;
    const RIGHT_SHIFT: i64 = 0x3C;
    const CONTROL: i64 = 0x3B;
    const RIGHT_COMMAND: i64 = 0x36;
    const FUNCTION: i64 = 0x3F;
    const CAPS_LOCK: i64 = 0x39;

    fn key(code: KeyMappingCode) -> Key {
        Key(code)
    }

    #[test]
    fn decodes_left_and_right_modifiers_from_device_bits() {
        assert_eq!(
            decode_flags_changed(SHIFT, NX_SHIFTMASK | NX_DEVICELSHIFTKEYMASK),
            Some(KeyEvent::Press(key(KeyMappingCode::ShiftLeft)))
        );
        // Left shift released while right shift is still held.
        assert_eq!(
            decode_flags_changed(SHIFT, NX_SHIFTMASK | NX_DEVICERSHIFTKEYMASK),
            Some(KeyEvent::Release(key(KeyMappingCode::ShiftLeft)))
        );
        assert_eq!(
            decode_flags_changed(RIGHT_SHIFT, NX_SHIFTMASK | NX_DEVICERSHIFTKEYMASK),
            Some(KeyEvent::Press(key(KeyMappingCode::ShiftRight)))
        );
        assert_eq!(
            decode_flags_changed(RIGHT_COMMAND, 0),
            Some(KeyEvent::Release(key(KeyMappingCode::MetaRight)))
        );
        assert_eq!(
            decode_flags_changed(FUNCTION, NX_SECONDARYFNMASK),
            Some(KeyEvent::Press(key(KeyMappingCode::Fn)))
        );
        assert_eq!(
            decode_flags_changed(FUNCTION, 0),
            Some(KeyEvent::Release(key(KeyMappingCode::Fn)))
        );
    }

    #[test]
    fn falls_back_to_generic_bits_for_virtual_keyboards() {
        assert_eq!(
            decode_flags_changed(CONTROL, NX_CONTROLMASK),
            Some(KeyEvent::Press(key(KeyMappingCode::ControlLeft)))
        );
        assert_eq!(
            decode_flags_changed(CONTROL, 0),
            Some(KeyEvent::Release(key(KeyMappingCode::ControlLeft)))
        );
    }

    #[test]
    fn caps_lock_and_unknown_keycodes_are_left_to_other_sources() {
        assert_eq!(decode_flags_changed(CAPS_LOCK, 0x1_0000), None);
        assert_eq!(decode_flags_changed(0x7FFF, NX_SHIFTMASK), None);
        assert_eq!(decode_flags_changed(-1, NX_SHIFTMASK), None);
    }

    #[test]
    fn maps_virtual_keycodes_to_chord_keys() {
        assert_eq!(key_from_keycode(0x31), Some(key(KeyMappingCode::Space)));
        assert_eq!(key_from_keycode(0x39), Some(key(KeyMappingCode::CapsLock)));
        assert_eq!(key_from_keycode(0x00), Some(key(KeyMappingCode::KeyA)));
        assert_eq!(key_from_keycode(0x7FFF), None);
    }
}
