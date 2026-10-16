//! Raw Caps Lock press/release input through IOHIDManager.
//! CGEventTap only observes the delayed toggle, so it cannot replace this source.
use super::health::{CAPS_DEVICE_GRACE, InputHealth};
use anyhow::{Context, Result, anyhow};
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFRunLoop, CFRunLoopRunResult, CFString, CFType,
    kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOHIDDevice, IOHIDManager, IOHIDValue, kHIDPage_GenericDesktop, kHIDPage_KeyboardOrKeypad,
    kHIDUsage_GD_Keyboard, kHIDUsage_KeyboardCapsLock, kIOReturnNotOpen,
};
use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const KIO_RETURN_SUCCESS: i32 = 0;
const KIO_RETURN_NO_DEVICE: i32 = 0xE000_02C0_u32 as i32;
const KIO_RETURN_EXCLUSIVE_ACCESS: i32 = 0xE000_02C5_u32 as i32;
const KIO_RETURN_NOT_PERMITTED: i32 = 0xE000_02E2_u32 as i32;

pub type CapsSink = Box<dyn FnMut(bool) + Send>;

unsafe extern "C" {
    fn mach_error_string(error: c_int) -> *const c_char;
}

/// Keeps successful devices alive until removal, without double-counting matches.
#[derive(Default)]
struct OpenDevices {
    // CFType uses CFEqual/CFHash, matching the original CFSet's device identity.
    devices: HashSet<CFRetained<CFType>>,
}

impl OpenDevices {
    fn matched(&mut self, device: CFRetained<CFType>, result: i32) {
        if result == KIO_RETURN_SUCCESS {
            self.devices.insert(device);
        }
    }

    fn removed(&mut self, device: &CFRetained<CFType>) -> bool {
        self.devices.remove(device)
    }

    fn len(&self) -> usize {
        self.devices.len()
    }
}

struct CapsContext {
    health: Arc<InputHealth>,
    sink: CapsSink,
    stop: Arc<AtomicBool>,
    devices: OpenDevices,
}

fn device_key(device: NonNull<IOHIDDevice>) -> CFRetained<CFType> {
    // SAFETY: IOKit lends a live CFType-compatible device during its callbacks.
    unsafe { CFRetained::cast_unchecked(CFRetained::retain(device)) }
}

/// The context is boxed by the listener thread and all callbacks run on that
/// thread's run loop. No Rust panic may escape into IOKit.
fn with_context(pointer: *mut c_void, callback: impl FnOnce(&mut CapsContext)) {
    let Some(context) = (unsafe { pointer.cast::<CapsContext>().as_mut() }) else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| callback(context))).is_err() {
        context.stop.store(true, Ordering::Release);
        let _ = catch_unwind(|| {
            log::error!("Caps Lock callback panicked; stopping the HID listener");
        });
    }
}

unsafe extern "C-unwind" fn device_matched(
    context: *mut c_void,
    result: i32,
    _sender: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    with_context(context, |state| {
        state.devices.matched(device_key(device), result);
        let device = unsafe { device.as_ref() };
        let product = product_name(device);
        let count = state.devices.len();
        state.health.set_caps_open_devices(count, Instant::now());
        if result == KIO_RETURN_SUCCESS {
            log::info!("Caps Lock source opened: \"{product}\" ({count} readable keyboard(s))");
        } else {
            log::warn!(
                "Caps Lock source skipped: \"{product}\": {}",
                describe_io_return(result)
            );
        }
    });
}

unsafe extern "C-unwind" fn device_removed(
    context: *mut c_void,
    result: i32,
    _sender: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    with_context(context, |state| {
        let key = device_key(device);
        if !state.devices.removed(&key) {
            return;
        }
        let product = product_name(unsafe { device.as_ref() });
        let count = state.devices.len();
        state.health.set_caps_open_devices(count, Instant::now());
        if count == 0 {
            log::warn!(
                "Caps Lock source removed: \"{product}\"; no readable keyboard left, input ownership is released unless one reappears within {CAPS_DEVICE_GRACE:?} ({})",
                describe_io_return(result)
            );
        } else {
            log::info!(
                "Caps Lock source removed: \"{product}\" ({count} readable keyboard(s) left)"
            );
        }
    });
}

unsafe extern "C-unwind" fn input_value(
    context: *mut c_void,
    result: i32,
    _sender: *mut c_void,
    value: NonNull<IOHIDValue>,
) {
    if result != KIO_RETURN_SUCCESS {
        return;
    }
    let value = unsafe { value.as_ref() };
    let element = value.element();
    if element.usage_page() != kHIDPage_KeyboardOrKeypad
        || element.usage() != kHIDUsage_KeyboardCapsLock
    {
        return;
    }
    with_context(context, |state| (state.sink)(value.integer_value() != 0));
}

fn product_name(device: &IOHIDDevice) -> String {
    device
        .property(&CFString::from_static_str("Product"))
        .and_then(|value| value.downcast::<CFString>().ok())
        .map(|name| name.to_string())
        .unwrap_or_else(|| "unknown keyboard".into())
}

/// Unregister callbacks before the boxed context is dropped, on every exit path.
struct HidSession {
    manager: CFRetained<IOHIDManager>,
    run_loop: CFRetained<CFRunLoop>,
}

impl Drop for HidSession {
    fn drop(&mut self) {
        unsafe {
            self.manager
                .register_input_value_callback(None, std::ptr::null_mut());
            self.manager
                .register_device_matching_callback(None, std::ptr::null_mut());
            self.manager
                .register_device_removal_callback(None, std::ptr::null_mut());
            if let Some(mode) = kCFRunLoopDefaultMode {
                self.manager.unschedule_from_run_loop(&self.run_loop, mode);
            }
        }
        self.manager.close(0);
    }
}

fn listen(context: &mut CapsContext, opened_tx: SyncSender<i32>) -> Result<i32> {
    let run_loop = CFRunLoop::current().context("No Caps Lock listener run loop")?;
    let mode = unsafe { kCFRunLoopDefaultMode }.context("No default run loop mode")?;
    let manager = IOHIDManager::new(None, 0);
    let matching = CFDictionary::from_slices(
        &[
            &*CFString::from_static_str("DeviceUsagePage"),
            &*CFString::from_static_str("DeviceUsage"),
        ],
        &[
            &*CFNumber::new_i32(kHIDPage_GenericDesktop as i32),
            &*CFNumber::new_i32(kHIDUsage_GD_Keyboard as i32),
        ],
    );
    // SAFETY: matching contains the documented numeric keyboard usage keys.
    unsafe { manager.set_device_matching(Some(matching.as_opaque())) };
    let session = HidSession { manager, run_loop };
    let pointer = (context as *mut CapsContext).cast();
    // SAFETY: the boxed context stays live until after session unregisters all
    // callbacks; scheduling and callbacks are confined to this listener thread.
    unsafe {
        session
            .manager
            .register_device_matching_callback(Some(device_matched), pointer);
        session
            .manager
            .register_device_removal_callback(Some(device_removed), pointer);
        session
            .manager
            .register_input_value_callback(Some(input_value), pointer);
        session
            .manager
            .schedule_with_run_loop(&session.run_loop, mode);
    }
    let result = session.manager.open(0);
    match result {
        KIO_RETURN_SUCCESS => log::info!("Caps Lock HID manager opened"),
        KIO_RETURN_EXCLUSIVE_ACCESS | KIO_RETURN_NO_DEVICE => log::info!(
            "Caps Lock HID manager opened with a partial result ({}); waiting for readable keyboards",
            describe_io_return(result)
        ),
        _ => {}
    }
    let _ = opened_tx.send(result);
    if !matches!(
        result,
        KIO_RETURN_SUCCESS | KIO_RETURN_EXCLUSIVE_ACCESS | KIO_RETURN_NO_DEVICE
    ) {
        return Ok(result);
    }
    while !context.stop.load(Ordering::Acquire) {
        if CFRunLoop::run_in_mode(Some(mode), 0.25, false) == CFRunLoopRunResult::Finished {
            return Ok(kIOReturnNotOpen as i32);
        }
    }
    Ok(KIO_RETURN_SUCCESS)
}

/// Human-readable IOReturn, retaining hints for exclusive access and permissions.
pub(super) fn describe_io_return(io_return: i32) -> String {
    let message = unsafe { mach_error_string(io_return) };
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    let hint = match io_return {
        KIO_RETURN_EXCLUSIVE_ACCESS => {
            " (another process, typically Karabiner-Elements, has seized this device; Chord reads Caps Lock from the devices that do open)"
        }
        KIO_RETURN_NOT_PERMITTED => {
            " (Input Monitoring is not granted to this executable; a rebuilt or terminal-launched binary may not be covered)"
        }
        _ => "",
    };
    format!("{message} [{:#010x}]{hint}", io_return as u32)
}

pub struct CapsHandle {
    thread: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl CapsHandle {
    pub fn spawn(health: Arc<InputHealth>, sink: CapsSink) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened_rx) = sync_channel::<i32>(1);
        let thread = std::thread::Builder::new()
            .name("chord-caps-hid".into())
            .spawn({
                let stop = stop.clone();
                move || run_listener(health, sink, stop, opened_tx)
            })
            .context("spawn Caps Lock listener thread")?;
        match opened_rx.recv_timeout(START_TIMEOUT) {
            Ok(KIO_RETURN_SUCCESS | KIO_RETURN_EXCLUSIVE_ACCESS | KIO_RETURN_NO_DEVICE) => {
                Ok(Self { thread, stop })
            }
            Ok(io_return) => {
                let _ = thread.join();
                Err(anyhow!(
                    "IOHIDManagerOpen failed: {}",
                    describe_io_return(io_return)
                ))
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                Err(anyhow!(
                    "Caps Lock listener did not open the HID manager within {START_TIMEOUT:?}"
                ))
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

fn run_listener(
    health: Arc<InputHealth>,
    sink: CapsSink,
    stop: Arc<AtomicBool>,
    opened_tx: SyncSender<i32>,
) {
    let mut context = Box::new(CapsContext {
        health: health.clone(),
        sink,
        stop: stop.clone(),
        devices: OpenDevices::default(),
    });
    let started = Instant::now();
    health.set_caps_running(true, started);
    let result = listen(&mut context, opened_tx);
    health.set_caps_running(false, Instant::now());
    if stop.load(Ordering::Acquire) {
        log::info!(
            "Caps Lock listener stopped on request after {:?}",
            started.elapsed()
        );
    } else {
        log::error!(
            "Caps Lock listener stopped after {:?}: {:?}",
            started.elapsed(),
            result.map(describe_io_return)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn describes_the_failures_that_matter() {
        let exclusive = describe_io_return(KIO_RETURN_EXCLUSIVE_ACCESS);
        assert!(exclusive.contains("0xe00002c5") && exclusive.contains("Karabiner"));
        assert!(describe_io_return(KIO_RETURN_NOT_PERMITTED).contains("Input Monitoring"));
        assert!(describe_io_return(0).contains("0x00000000"));
    }

    #[test]
    fn exclusive_devices_duplicates_and_hot_plug_preserve_readiness() {
        // Strings exercise the same CFEqual/CFHash registry without opening hardware.
        let key = |name: &str| unsafe {
            CFRetained::<CFString>::cast_unchecked::<CFType>(CFString::from_str(name))
        };
        let mut devices = OpenDevices::default();
        devices.matched(key("seized keyboard"), KIO_RETURN_EXCLUSIVE_ACCESS);
        assert_eq!(devices.len(), 0);
        devices.matched(key("virtual keyboard"), 0);
        devices.matched(key("virtual keyboard"), 0);
        devices.matched(key("external keyboard"), 0);
        assert_eq!(devices.len(), 2);
        assert!(!devices.removed(&key("seized keyboard")));
        assert!(devices.removed(&key("virtual keyboard")));
        assert_eq!(devices.len(), 1);
        assert!(devices.removed(&key("external keyboard")));
        assert!(!devices.removed(&key("external keyboard")));
        devices.matched(key("denied keyboard"), KIO_RETURN_NOT_PERMITTED);
        assert_eq!(devices.len(), 0);
        devices.matched(key("virtual keyboard"), 0);
        assert_eq!(devices.len(), 1);
    }

    #[test]
    fn callback_panics_stop_the_listener_without_crossing_iokit() {
        let stop = Arc::new(AtomicBool::new(false));
        let mut context = CapsContext {
            health: Arc::new(InputHealth::default()),
            sink: Box::new(|_| panic!("fixture sink")),
            stop: stop.clone(),
            devices: OpenDevices::default(),
        };
        with_context((&mut context as *mut CapsContext).cast(), |state| {
            (state.sink)(true)
        });
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    #[ignore = "opens real keyboard devices; requires Input Monitoring"]
    fn native_listener_stops_and_restarts() {
        for _ in 0..2 {
            let handle =
                CapsHandle::spawn(Arc::new(InputHealth::default()), Box::new(|_| {})).unwrap();
            handle.stop();
        }
    }
}
