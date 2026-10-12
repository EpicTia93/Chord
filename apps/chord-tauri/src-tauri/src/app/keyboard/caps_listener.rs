//! Caps Lock listener built on the IOHIDManager code in `native/hid.c`.
//!
//! Every device event the native side sees is logged here, because the usual
//! reasons Caps Lock goes quiet are environmental and otherwise invisible:
//! Karabiner-Elements seizing the physical keyboard and recreating its virtual
//! one, Input Monitoring not covering the running executable, or the
//! keyboards vanishing across sleep.

use super::health::{CAPS_DEVICE_GRACE, InputHealth};
use anyhow::{Context, Result, anyhow};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(10);

const KIO_RETURN_SUCCESS: i32 = 0;
const KIO_RETURN_NO_DEVICE: i32 = 0xE000_02C0_u32 as i32;
const KIO_RETURN_EXCLUSIVE_ACCESS: i32 = 0xE000_02C5_u32 as i32;
const KIO_RETURN_NOT_PERMITTED: i32 = 0xE000_02E2_u32 as i32;

const DEVICE_OPENED: c_int = 0;
const DEVICE_OPEN_FAILED: c_int = 1;
const DEVICE_REMOVED: c_int = 2;

pub type CapsSink = Box<dyn FnMut(bool) + Send>;

#[repr(C)]
struct Callbacks {
    context: *mut c_void,
    on_caps: extern "C" fn(*mut c_void, c_int),
    on_device: extern "C" fn(*mut c_void, c_int, i32, *const c_char, c_int),
    on_manager_opened: extern "C" fn(*mut c_void, i32),
}

unsafe extern "C" {
    fn chord_caps_listener_run(callbacks: *const Callbacks, stop_flag: *const c_int) -> i32;
    fn mach_error_string(error: c_int) -> *const c_char;
}

struct CapsContext {
    health: Arc<InputHealth>,
    sink: CapsSink,
    manager_opened: Option<SyncSender<i32>>,
}

extern "C" fn on_caps(context: *mut c_void, pressed: c_int) {
    // SAFETY: the context is the boxed `CapsContext` owned by the listener thread.
    let Some(context) = (unsafe { context.cast::<CapsContext>().as_mut() }) else {
        return;
    };
    (context.sink)(pressed != 0);
}

extern "C" fn on_device(
    context: *mut c_void,
    kind: c_int,
    io_return: i32,
    product: *const c_char,
    open_count: c_int,
) {
    // SAFETY: see `on_caps`; `product` is a NUL-terminated buffer owned by the caller.
    let Some(context) = (unsafe { context.cast::<CapsContext>().as_mut() }) else {
        return;
    };
    let product = if product.is_null() {
        String::from("unknown keyboard")
    } else {
        unsafe { CStr::from_ptr(product) }.to_string_lossy().into_owned()
    };
    let open_count = usize::try_from(open_count).unwrap_or(0);
    context.health.set_caps_open_devices(open_count, Instant::now());
    match kind {
        DEVICE_OPENED => log::info!(
            "Caps Lock source opened: \"{product}\" ({open_count} readable keyboard(s))"
        ),
        DEVICE_OPEN_FAILED => log::warn!(
            "Caps Lock source skipped: \"{product}\": {}",
            describe_io_return(io_return)
        ),
        DEVICE_REMOVED if open_count == 0 => log::warn!(
            "Caps Lock source removed: \"{product}\"; no readable keyboard left, input ownership is released unless one reappears within {CAPS_DEVICE_GRACE:?}"
        ),
        DEVICE_REMOVED => log::info!(
            "Caps Lock source removed: \"{product}\" ({open_count} readable keyboard(s) left)"
        ),
        _ => log::warn!("Unknown Caps Lock device event {kind} for \"{product}\""),
    }
}

extern "C" fn on_manager_opened(context: *mut c_void, io_return: i32) {
    // SAFETY: see `on_caps`.
    let Some(context) = (unsafe { context.cast::<CapsContext>().as_mut() }) else {
        return;
    };
    match io_return {
        KIO_RETURN_SUCCESS => log::info!("Caps Lock HID manager opened"),
        KIO_RETURN_EXCLUSIVE_ACCESS | KIO_RETURN_NO_DEVICE => log::info!(
            "Caps Lock HID manager opened with a partial result ({}); waiting for readable keyboards",
            describe_io_return(io_return)
        ),
        _ => {}
    }
    if let Some(manager_opened) = context.manager_opened.take() {
        let _ = manager_opened.send(io_return);
    }
}

/// Human-readable IOReturn, with a hint for the two failures that matter here.
pub(super) fn describe_io_return(io_return: i32) -> String {
    let message = unsafe { mach_error_string(io_return) };
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
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

/// A running Caps Lock listener thread. Call [`CapsHandle::stop`] to end it.
pub struct CapsHandle {
    thread: JoinHandle<()>,
    stop: Arc<AtomicI32>,
}

impl CapsHandle {
    /// Starts the listener and returns once the HID manager is open.
    pub fn spawn(health: Arc<InputHealth>, sink: CapsSink) -> Result<Self> {
        let stop = Arc::new(AtomicI32::new(0));
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
                // Includes a thread that died before reporting (the sender drops).
                stop.store(1, Ordering::Release);
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
        self.stop.store(1, Ordering::Release);
        let _ = self.thread.join();
    }
}

fn run_listener(
    health: Arc<InputHealth>,
    sink: CapsSink,
    stop: Arc<AtomicI32>,
    opened_tx: SyncSender<i32>,
) {
    let mut context = Box::new(CapsContext {
        health: health.clone(),
        sink,
        manager_opened: Some(opened_tx),
    });
    let callbacks = Callbacks {
        context: (&mut *context as *mut CapsContext).cast(),
        on_caps,
        on_device,
        on_manager_opened,
    };
    let started = Instant::now();
    health.set_caps_running(true, started);
    // SAFETY: `callbacks` and `stop` outlive the call; the native side only
    // uses the context pointer from within this call on this thread.
    let result = unsafe { chord_caps_listener_run(&callbacks, stop.as_ptr()) };
    health.set_caps_running(false, Instant::now());
    if stop.load(Ordering::Acquire) != 0 {
        log::info!(
            "Caps Lock listener stopped on request after {:?}",
            started.elapsed()
        );
    } else {
        log::error!(
            "Caps Lock listener stopped after {:?}: {}",
            started.elapsed(),
            describe_io_return(result)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_the_failures_that_matter() {
        let exclusive = describe_io_return(KIO_RETURN_EXCLUSIVE_ACCESS);
        assert!(exclusive.contains("0xe00002c5"), "{exclusive}");
        assert!(exclusive.contains("Karabiner"), "{exclusive}");
        let denied = describe_io_return(KIO_RETURN_NOT_PERMITTED);
        assert!(denied.contains("Input Monitoring"), "{denied}");
        assert!(describe_io_return(0).contains("0x00000000"));
    }
}
