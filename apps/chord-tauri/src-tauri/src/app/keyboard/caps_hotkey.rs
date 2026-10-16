//! Keeps the remapped Caps key working where Chord's event tap is blind.
//!
//! Secure Event Input (password fields, Secure Keyboard Entry) withholds keyboard
//! events from every event tap, but WindowServer still delivers Carbon hotkeys. While
//! the tap sees the remapped key it swallows it, so this hotkey only fires when the
//! tap cannot: no Secure Input detection is needed. Chord modes can't start without
//! the tap, so a press here is always a plain Caps Lock tap.

use crate::app::AppHandleExt;
use crate::models::{Key, KeyEvent};
use anyhow::{Result, ensure};
use keycode::KeyMappingCode;
use std::ffi::c_void;
use std::sync::OnceLock;
use tauri::AppHandle;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHotKeyRef = *mut c_void;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandler,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
    fn GetEventKind(event: EventRef) -> u32;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_HOT_KEY_RELEASED: u32 = 6;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const EVENT_NOT_HANDLED_ERR: OSStatus = -9874;
const SIGNATURE: u32 = u32::from_be_bytes(*b"CHRD");
/// Carbon modifier masks the hotkey is registered for: none and Shift (`shiftKey`),
/// since a hotkey only matches its exact modifiers.
const MODIFIER_VARIANTS: [u32; 2] = [0, 1 << 9];

static HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Registered hotkeys. Only touched on the main thread; stored as addresses.
static REGISTERED: parking_lot::Mutex<Vec<usize>> = parking_lot::Mutex::new(Vec::new());

extern "C" fn handle_hotkey(_: EventHandlerCallRef, event: EventRef, _: *mut c_void) -> OSStatus {
    let mut id = EventHotKeyID {
        signature: 0,
        id: 0,
    };
    // SAFETY: Carbon passes a live event for the duration of the callback, and the
    // output is exactly one hotkey ID.
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            (&mut id as *mut EventHotKeyID).cast(),
        )
    };
    // Other hotkeys in this process belong to other handlers.
    let (true, Some(handle)) = (status == 0 && id.signature == SIGNATURE, HANDLE.get()) else {
        return EVENT_NOT_HANDLED_ERR;
    };
    let key = Key(KeyMappingCode::CapsLock);
    // SAFETY: as above.
    let event = match unsafe { GetEventKind(event) } {
        K_EVENT_HOT_KEY_PRESSED => KeyEvent::Press(key),
        K_EVENT_HOT_KEY_RELEASED => KeyEvent::Release(key),
        _ => return EVENT_NOT_HANDLED_ERR,
    };
    log::debug!("Caps hotkey fallback: {event:?} (the event tap cannot see the key)");
    handle
        .app_state()
        .keyboard()
        .handle_unseen_caps_event(&event);
    0
}

/// Registers or unregisters the fallback. Must run on the main thread.
pub(super) fn set_registered(handle: &AppHandle, registered: bool) -> Result<()> {
    let mut hotkeys = REGISTERED.lock();
    if registered == !hotkeys.is_empty() {
        return Ok(());
    }
    if !registered {
        for hotkey in hotkeys.drain(..) {
            // SAFETY: registered by this module on the main thread.
            unsafe { UnregisterEventHotKey(hotkey as EventHotKeyRef) };
        }
        return Ok(());
    }
    if HANDLE.set(handle.clone()).is_ok() {
        let types = [
            EventTypeSpec {
                event_class: K_EVENT_CLASS_KEYBOARD,
                event_kind: K_EVENT_HOT_KEY_PRESSED,
            },
            EventTypeSpec {
                event_class: K_EVENT_CLASS_KEYBOARD,
                event_kind: K_EVENT_HOT_KEY_RELEASED,
            },
        ];
        // SAFETY: a static handler and a stack array Carbon copies.
        let status = unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                handle_hotkey,
                types.len() as u32,
                types.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        ensure!(status == 0, "InstallEventHandler failed: {status}");
    }
    for (index, modifiers) in MODIFIER_VARIANTS.into_iter().enumerate() {
        let mut hotkey: EventHotKeyRef = std::ptr::null_mut();
        // SAFETY: the output points to live storage; the target is the app's.
        let status = unsafe {
            RegisterEventHotKey(
                super::caps_remap::REMAPPED_KEYCODE as u32,
                modifiers,
                EventHotKeyID {
                    signature: SIGNATURE,
                    id: index as u32 + 1,
                },
                GetApplicationEventTarget(),
                0,
                &mut hotkey,
            )
        };
        if status != 0 {
            log::warn!(
                "Could not register the Caps fallback hotkey (modifiers {modifiers:#x}): {status}"
            );
            continue;
        }
        hotkeys.push(hotkey as usize);
    }
    Ok(())
}
