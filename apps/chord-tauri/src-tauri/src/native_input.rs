//! Window-targeted mouse input shared by Rust callers and the built-in `chord` module.
//! Coordinates are top-left window-local macOS points. No cursor movement or activation.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl WindowBounds {
    fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|n| n.is_finite())
            && self.width >= 1.0
            && self.height >= 1.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeWindow {
    pub window_id: u32,
    pub process_id: i32,
    pub bounds: WindowBounds,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClickOptions {
    pub window_id: u32,
    pub x: f64,
    pub y: f64,
    /// If supplied, reject a window that moved or resized since the caller resolved coordinates.
    pub expected_bounds: Option<WindowBounds>,
}

impl ClickOptions {
    fn validate(&self) -> Result<()> {
        ensure!(self.window_id != 0, "windowId must be a nonzero window ID");
        ensure!(
            self.x.is_finite() && self.y.is_finite() && self.x >= 0.0 && self.y >= 0.0,
            "click coordinates must be finite nonnegative window-local points"
        );
        if let Some(bounds) = self.expected_bounds {
            ensure!(
                bounds.is_valid(),
                "expectedBounds must contain valid window bounds"
            );
        }
        Ok(())
    }

    fn validate_window(&self, window: &NativeWindow) -> Result<()> {
        self.validate()?;
        ensure!(window.bounds.is_valid(), "target window has invalid bounds");
        ensure!(
            self.x < window.bounds.width && self.y < window.bounds.height,
            "click coordinates are outside the target window"
        );
        if let Some(expected) = self.expected_bounds {
            ensure!(
                expected == window.bounds,
                "target window moved or resized; resolve coordinates again"
            );
        }
        Ok(())
    }
}

/// List the on-screen normal windows of an already-running application. Never launches it.
pub fn get_app_windows(bundle_id: &str) -> Result<Vec<NativeWindow>> {
    ensure!(!bundle_id.trim().is_empty(), "bundleId must not be empty");
    #[cfg(target_os = "macos")]
    return macos::get_app_windows(bundle_id);
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("window-targeted mouse input is only supported on macOS");
}

/// Send a left-button down/up pair to one window. Run off the UI/JS thread (holds for 80 ms).
pub fn click(options: &ClickOptions) -> Result<()> {
    options.validate()?;
    #[cfg(target_os = "macos")]
    return macos::click(options);
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("window-targeted mouse input is only supported on macOS");
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use anyhow::Context;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSRunningApplication};
    use objc2_core_foundation::{
        CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGPreflightPostEventAccess, CGWindowListCopyWindowInfo,
        CGWindowListOption, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerPID,
    };
    use objc2_foundation::{NSPoint, NSProcessInfo, NSString};
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::OnceLock;
    use std::time::Duration;

    type SetWindowLocation = unsafe extern "C" fn(*const CGEvent, CGPoint);

    fn window_location_setter() -> Result<SetWindowLocation> {
        static SETTER: OnceLock<Option<SetWindowLocation>> = OnceLock::new();
        let setter = SETTER.get_or_init(|| {
            // SAFETY: resolve the known Core Graphics SPI ABI; never call a missing symbol.
            let symbol =
                unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"CGEventSetWindowLocation".as_ptr()) };
            (!symbol.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, SetWindowLocation>(symbol) })
        });
        setter.context("CGEventSetWindowLocation is unavailable on this macOS version")
    }

    fn dictionary_value<'a>(dict: &'a CFDictionary, key: &CFString) -> Option<&'a CFType> {
        // SAFETY: window info is an immutable retained CF dictionary with CF object values.
        let value = unsafe { dict.value((key as *const CFString).cast()) };
        unsafe { value.cast::<CFType>().as_ref() }
    }

    fn number(dict: &CFDictionary, key: &CFString) -> Option<i64> {
        dictionary_value(dict, key)?
            .downcast_ref::<CFNumber>()?
            .as_i64()
    }

    fn windows() -> Result<Vec<NativeWindow>> {
        let list =
            CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)
                .context("could not enumerate on-screen windows")?;
        let mut windows = Vec::new();
        for index in 0..list.count() {
            // SAFETY: CGWindowListCopyWindowInfo returns an immutable array of CF dictionaries.
            let entry = unsafe { &*list.value_at_index(index).cast::<CFType>() };
            let Some(dict) = entry.downcast_ref::<CFDictionary>() else {
                continue;
            };
            // SAFETY: these are Core Graphics' exported immutable CFString constants.
            let (id, pid, layer, raw_bounds, title) = unsafe {
                (
                    number(dict, kCGWindowNumber),
                    number(dict, kCGWindowOwnerPID),
                    number(dict, kCGWindowLayer),
                    dictionary_value(dict, kCGWindowBounds),
                    dictionary_value(dict, kCGWindowName),
                )
            };
            if layer != Some(0) {
                continue;
            }
            let (Some(id), Some(pid), Some(raw_bounds)) = (id, pid, raw_bounds) else {
                continue;
            };
            let Some(bounds) = raw_bounds.downcast_ref::<CFDictionary>() else {
                continue;
            };
            let read = |key: &str| -> Option<f64> {
                dictionary_value(bounds, &CFString::from_str(key))?
                    .downcast_ref::<CFNumber>()?
                    .as_f64()
            };
            let (Some(x), Some(y), Some(width), Some(height)) =
                (read("X"), read("Y"), read("Width"), read("Height"))
            else {
                continue;
            };
            let (Ok(window_id), Ok(process_id)) = (u32::try_from(id), i32::try_from(pid)) else {
                continue;
            };
            windows.push(NativeWindow {
                window_id,
                process_id,
                bounds: WindowBounds {
                    x,
                    y,
                    width,
                    height,
                },
                title: title
                    .and_then(|v| v.downcast_ref::<CFString>())
                    .map(ToString::to_string),
            });
        }
        Ok(windows)
    }

    pub(super) fn get_app_windows(bundle_id: &str) -> Result<Vec<NativeWindow>> {
        autoreleasepool(|_| {
            let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(bundle_id),
            );
            let pids: Vec<_> = apps.iter().map(|app| app.processIdentifier()).collect();
            Ok(windows()?
                .into_iter()
                .filter(|window| pids.contains(&window.process_id))
                .collect())
        })
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut CFType;
        fn AXUIElementCopyAttributeValue(
            element: *const CFType,
            attribute: *const CFString,
            value: *mut *mut CFType,
        ) -> i32;
    }

    fn is_frontmost(pid: i32) -> Result<bool> {
        // AX is a fresh query; NSWorkspace.frontmostApplication can be stale on worker threads.
        // SAFETY: these create/copy APIs transfer owned CF objects, released by CFRetained.
        let application = NonNull::new(unsafe { AXUIElementCreateApplication(pid) })
            .context("could not inspect target application's focus")?;
        let application = unsafe { CFRetained::from_raw(application) };
        let attribute = CFString::from_static_str("AXFrontmost");
        let mut value = std::ptr::null_mut();
        let status =
            unsafe { AXUIElementCopyAttributeValue(&*application, &*attribute, &mut value) };
        // Adopt even on failure so a non-null result is always released.
        let value = NonNull::new(value).map(|raw| unsafe { CFRetained::from_raw(raw) });
        ensure!(
            status == 0,
            "could not read target application's focus (AX error {status})"
        );
        value
            .and_then(|v| v.downcast::<CFBoolean>().ok())
            .map(|v| v.as_bool())
            .context("target application returned an invalid focus value")
    }

    fn mouse_event(
        window: &NativeWindow,
        x: f64,
        y: f64,
        down: bool,
        background: bool,
        setter: SetWindowLocation,
    ) -> Result<CFRetained<CGEvent>> {
        let local = CGPoint::new(x, y);
        let global = CGPoint::new(window.bounds.x + x, window.bounds.y + y);
        let event_type = if down {
            NSEventType::LeftMouseDown
        } else {
            NSEventType::LeftMouseUp
        };
        let flags = if background {
            NSEventModifierFlags::Command
        } else {
            NSEventModifierFlags::empty()
        };
        let ns_event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            event_type, NSPoint::new(x, y), flags, NSProcessInfo::processInfo().systemUptime(),
            window.window_id as isize, None, 1, 1, if down { 1.0 } else { 0.0 },
        ).context("could not create a window-targeted mouse event")?;
        let source = ns_event
            .CGEvent()
            .context("could not convert the window-targeted mouse event")?;
        let event = CGEvent::new_copy(Some(&source))
            .context("could not copy the window-targeted mouse event")?;
        CGEvent::set_location(Some(&event), global);
        CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventButtonNumber, 0);
        // Subtype 3 and Command on background events are the verified non-activating recipe.
        CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventSubtype, 3);
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventWindowUnderMousePointer,
            window.window_id.into(),
        );
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
            window.window_id.into(),
        );
        // SAFETY: the SPI was resolved with its actual ABI and `event` is a live mutable copy.
        unsafe { setter(&*event, local) };
        Ok(event)
    }

    pub(super) fn click(options: &ClickOptions) -> Result<()> {
        autoreleasepool(|_| {
            let setter = window_location_setter()?;
            ensure!(
                CGPreflightPostEventAccess(),
                "Accessibility permission is required for window-targeted clicks"
            );
            let window = windows()?
                .into_iter()
                .find(|window| window.window_id == options.window_id)
                .context("target window is no longer on-screen")?;
            options.validate_window(&window)?;
            let background = !is_frontmost(window.process_id)?;
            // Construct both events before posting either so construction failure cannot leave a button held.
            let down = mouse_event(&window, options.x, options.y, true, background, setter)?;
            let up = mouse_event(&window, options.x, options.y, false, background, setter)?;
            CGEvent::post_to_pid(window.process_id, Some(&down));
            std::thread::sleep(Duration::from_millis(80));
            CGEvent::post_to_pid(window.process_id, Some(&up));
            Ok(())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn constructs_window_targeted_events_without_posting_input() {
            type GetWindowLocation = unsafe extern "C" fn(*const CGEvent) -> CGPoint;
            // Read the stamped field directly: NSEvent's locationInWindow falls back to
            // screen coordinates when our deliberately fake window ID has no real NSWindow.
            let getter =
                unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"CGEventGetWindowLocation".as_ptr()) };
            assert!(!getter.is_null());
            let getter = unsafe { std::mem::transmute::<*mut c_void, GetWindowLocation>(getter) };
            let window = NativeWindow {
                window_id: 123,
                process_id: 456,
                bounds: WindowBounds {
                    x: -500.0,
                    y: 200.0,
                    width: 300.0,
                    height: 600.0,
                },
                title: None,
            };
            let setter = window_location_setter().unwrap();
            for background in [false, true] {
                for down in [false, true] {
                    let event =
                        mouse_event(&window, 30.5, 40.25, down, background, setter).unwrap();
                    let decoded = NSEvent::eventWithCGEvent(&event).unwrap();
                    assert_eq!(decoded.windowNumber(), 123);
                    assert_eq!(unsafe { getter(&*event) }, CGPoint::new(30.5, 40.25));
                    assert_eq!(
                        CGEvent::location(Some(&event)),
                        CGPoint::new(-469.5, 240.25)
                    );
                    assert_eq!(
                        CGEvent::integer_value_field(Some(&event), CGEventField::MouseEventSubtype),
                        3
                    );
                    for field in [
                        CGEventField::MouseEventWindowUnderMousePointer,
                        CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
                    ] {
                        assert_eq!(CGEvent::integer_value_field(Some(&event), field), 123);
                    }
                    assert_eq!(
                        decoded
                            .modifierFlags()
                            .contains(NSEventModifierFlags::Command),
                        background
                    );
                    assert_eq!(
                        decoded.r#type(),
                        if down {
                            NSEventType::LeftMouseDown
                        } else {
                            NSEventType::LeftMouseUp
                        }
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> NativeWindow {
        NativeWindow {
            window_id: 123,
            process_id: 456,
            bounds: WindowBounds {
                x: -500.0,
                y: 200.0,
                width: 300.0,
                height: 600.0,
            },
            title: None,
        }
    }

    #[test]
    fn validates_coordinates_before_any_native_input() {
        let window = window();
        for (x, y) in [(0.0, 0.0), (299.5, 599.5)] {
            assert!(
                ClickOptions {
                    window_id: 123,
                    x,
                    y,
                    expected_bounds: Some(window.bounds)
                }
                .validate_window(&window)
                .is_ok()
            );
        }
        for (x, y) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (-1.0, 0.0),
            (300.0, 0.0),
            (0.0, 600.0),
        ] {
            assert!(
                ClickOptions {
                    window_id: 123,
                    x,
                    y,
                    expected_bounds: None
                }
                .validate_window(&window)
                .is_err()
            );
        }
        assert!(
            click(&ClickOptions {
                window_id: 0,
                x: 0.0,
                y: 0.0,
                expected_bounds: None
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_moved_or_resized_windows() {
        let window = window();
        for bounds in [
            WindowBounds {
                x: -499.0,
                ..window.bounds
            },
            WindowBounds {
                height: 601.0,
                ..window.bounds
            },
        ] {
            assert!(
                ClickOptions {
                    window_id: 123,
                    x: 1.0,
                    y: 1.0,
                    expected_bounds: Some(bounds)
                }
                .validate_window(&window)
                .is_err()
            );
        }
    }
}
