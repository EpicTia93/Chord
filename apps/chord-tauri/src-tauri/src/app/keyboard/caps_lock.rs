//! Caps Lock toggle state through IOKit's HID system connection.
use anyhow::{Context, Result, ensure};
use objc2_io_kit::{
    IOHIDGetModifierLockState, IOHIDSetModifierLockState, IOObjectRelease,
    IOServiceClose, IOServiceGetMatchingService, IOServiceMatching, IOServiceOpen,
    kIOHIDCapsLockState, kIOHIDParamConnectType, kIOMainPortDefault,
};

struct HidSystem(u32);

impl HidSystem {
    fn open() -> Result<Self> {
        // SAFETY: a static class name; the matching-service API consumes the
        // dictionary's owned reference, including when no service is found.
        let matching = unsafe { IOServiceMatching(c"IOHIDSystem".as_ptr()) }
            .context("Could not create IOHIDSystem matching dictionary")?;
        let service = unsafe { IOServiceGetMatchingService(kIOMainPortDefault, Some(matching)) };
        ensure!(service != 0, "Failed to find IOHIDSystem");
        let mut connection = 0;
        // SAFETY: service is an owned IOKit handle; the output points to live storage.
        let result = unsafe {
            IOServiceOpen(service, libc::mach_task_self(), kIOHIDParamConnectType, &mut connection)
        };
        IOObjectRelease(service);
        ensure!(result == 0, "IOServiceOpen failed: {:#010x}", result as u32);
        Ok(Self(connection))
    }

    fn read(&self) -> Result<bool> {
        let mut enabled = false;
        // SAFETY: the connection stays open and the output is a valid bool pointer.
        let result = unsafe {
            IOHIDGetModifierLockState(self.0, kIOHIDCapsLockState as i32, &mut enabled)
        };
        ensure!(result == 0, "IOHIDGetModifierLockState failed: {:#010x}", result as u32);
        Ok(enabled)
    }

    fn write(&self, enabled: bool) -> Result<()> {
        let result = IOHIDSetModifierLockState(self.0, kIOHIDCapsLockState as i32, enabled);
        ensure!(result == 0, "IOHIDSetModifierLockState failed: {:#010x}", result as u32);
        Ok(())
    }
}

impl Drop for HidSystem {
    fn drop(&mut self) {
        IOServiceClose(self.0);
    }
}

pub(super) fn get() -> Result<bool> {
    HidSystem::open()?.read()
}

pub(super) fn set(enabled: bool) -> Result<()> {
    HidSystem::open()?.write(enabled)
}

pub(super) fn toggle() -> Result<()> {
    // Preserve the original read-then-write behavior and propagate either failure.
    set(!get()?)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "reads the real IOHIDSystem; requires an active macOS desktop"]
    fn reads_native_caps_lock_without_changing_it() {
        super::get().expect("read Caps Lock state through IOKit");
    }
}
