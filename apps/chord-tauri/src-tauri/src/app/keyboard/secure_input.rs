//! Secure Event Input detection.
//!
//! While any process has secure input enabled (a password field, Terminal's
//! "Secure Keyboard Entry", a password manager that forgot to turn it off),
//! macOS withholds keyboard events from every other process's event tap.
//! Chord cannot work around that; it can only say so in the log instead of
//! looking like it randomly stopped.

use objc2_core_foundation::{CFNumber, CFNumberType, CFString, CFType};
use std::ffi::c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SecureInputState {
    #[default]
    Disabled,
    Enabled {
        pid: Option<i32>,
        process: Option<String>,
    },
}

impl SecureInputState {
    pub fn current() -> Self {
        // SAFETY: plain Carbon query without arguments.
        if unsafe { IsSecureEventInputEnabled() } == 0 {
            return Self::Disabled;
        }
        let pid = secure_input_pid();
        Self::Enabled {
            pid,
            process: pid.and_then(process_name),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Disabled => "disabled".into(),
            Self::Enabled { pid: None, .. } => "enabled by an unknown process".into(),
            Self::Enabled {
                pid: Some(pid),
                process,
            } => format!(
                "enabled by pid {pid} ({})",
                process.as_deref().unwrap_or("unknown process")
            ),
        }
    }
}

fn secure_input_pid() -> Option<i32> {
    let session = objc2_core_graphics::CGSessionCopyCurrentDictionary()?;
    let key = CFString::from_static_str("kCGSSessionSecureInputPID");
    // SAFETY: the key is a live CFString and the dictionary is retained.
    let value = unsafe { session.value((&*key as *const CFString).cast::<c_void>()) };
    if value.is_null() {
        return None;
    }
    // SAFETY: values of a session dictionary are CF objects owned by the dictionary.
    let value = unsafe { &*value.cast::<CFType>() };
    let number = value.downcast_ref::<CFNumber>()?;
    let mut pid: i32 = 0;
    // SAFETY: `pid` is a valid i32 for `kCFNumberSInt32Type`.
    unsafe {
        number.value(
            CFNumberType::SInt32Type,
            (&mut pid as *mut i32).cast::<c_void>(),
        )
    }
    .then_some(pid)
}

fn process_name(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for `buffer.len()` bytes.
    let length = unsafe {
        libc::proc_name(
            pid,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
        )
    };
    let length = usize::try_from(length).ok()?;
    (length > 0).then(|| String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned())
}
