//! Main-thread Accessibility and workspace observers, owned entirely by Rust.
use block2::RcBlock;
use objc2::{
    MainThreadMarker,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidDeactivateApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceDidWakeNotification,
};
use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFRetained, CFRunLoop, CFRunLoopSource, CFString, CFType, ConcreteType,
    kCFRunLoopCommonModes,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::NonNull,
    rc::Rc,
};

fn attribute<T: ConcreteType>(element: &AXUIElement, name: &'static str) -> Option<CFRetained<T>> {
    let mut raw: *const CFType = std::ptr::null();
    // SAFETY: the out parameter is live storage; copy transfers an owned CF value.
    let error = unsafe {
        element.copy_attribute_value(&CFString::from_static_str(name), NonNull::from(&mut raw))
    };
    let value =
        NonNull::new(raw.cast_mut()).map(|pointer| unsafe { CFRetained::from_raw(pointer) });
    if error == AXError::Success {
        value?.downcast::<T>().ok()
    } else {
        None
    }
}

fn is_picker(element: &AXUIElement) -> bool {
    attribute::<CFString>(element, "AXIdentifier").is_some_and(|identifier| {
        matches!(identifier.to_string().as_str(), "open-panel" | "save-panel")
    })
}

fn inside_picker(element: &AXUIElement) -> bool {
    let mut current = Some(unsafe { CFRetained::retain(NonNull::from(element)) });
    for _ in 0..32 {
        let Some(element) = current.take() else {
            break;
        };
        if is_picker(&element) {
            return true;
        }
        current = attribute(&element, "AXParent");
    }
    false
}

fn has_picker_sheet(window: &AXUIElement) -> bool {
    is_picker(window)
        || attribute::<CFArray>(window, "AXChildren").is_some_and(|children| {
            // AXChildren contains CF objects; individual element types are checked below.
            let children = unsafe { children.cast_unchecked::<CFType>() };
            children
                .iter()
                .any(|child| child.downcast_ref::<AXUIElement>().is_some_and(is_picker))
        })
}

struct ProcessObserver {
    _observer: CFRetained<AXObserver>,
    _application: CFRetained<AXUIElement>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

impl ProcessObserver {
    fn new(pid: i32, listener: &Rc<Listener>) -> Option<Self> {
        let application = unsafe { AXUIElement::new_application(pid) };
        unsafe { application.set_messaging_timeout(0.15) };
        let mut pointer = std::ptr::null_mut();
        let error = unsafe {
            AXObserver::create(
                pid,
                Some(accessibility_changed),
                NonNull::from(&mut pointer),
            )
        };
        let observer = NonNull::new(pointer).map(|p| unsafe { CFRetained::from_raw(p) })?;
        if error != AXError::Success {
            return None;
        }
        let mut subscribed = false;
        for name in [
            "AXWindowCreated",
            "AXSheetCreated",
            "AXFocusedWindowChanged",
            "AXFocusedUIElementChanged",
            "AXUIElementDestroyed",
        ] {
            // SAFETY: Listener owns this observer, removes its source before being
            // dropped, and both registration and callbacks are on the main thread.
            let result = unsafe {
                observer.add_notification(
                    &application,
                    &CFString::from_static_str(name),
                    Rc::as_ptr(listener).cast_mut().cast(),
                )
            };
            if result == AXError::Success {
                subscribed = true;
            } else if result != AXError::NotificationUnsupported {
                log::debug!("File picker subscription {name} for pid {pid} failed: {result:?}");
            }
        }
        if !subscribed {
            return None;
        }
        let source = unsafe { observer.run_loop_source() };
        let run_loop = CFRunLoop::main()?;
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        Some(Self {
            _observer: observer,
            _application: application,
            source,
            run_loop,
        })
    }
}

impl Drop for ProcessObserver {
    fn drop(&mut self) {
        self.run_loop
            .remove_source(Some(&self.source), unsafe { kCFRunLoopCommonModes });
    }
}

/// Rc and MainThreadMarker keep AppKit/AX resources on their owning thread.
/// Dropping the listener unregisters everything and clears cached context.
pub(super) struct Listener {
    _main_thread: MainThreadMarker,
    processes: RefCell<HashMap<i32, ProcessObserver>>,
    center: Retained<NSNotificationCenter>,
    tokens: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    pending: Cell<bool>,
    changed: Box<dyn Fn(bool)>,
}

impl Listener {
    pub(super) fn start(
        main_thread: MainThreadMarker,
        changed: impl Fn(bool) + 'static,
    ) -> Rc<Self> {
        let listener = Rc::new(Self {
            _main_thread: main_thread,
            processes: RefCell::new(HashMap::new()),
            center: NSWorkspace::sharedWorkspace().notificationCenter(),
            tokens: RefCell::new(Vec::new()),
            pending: Cell::new(false),
            changed: Box::new(changed),
        });
        let queue = NSOperationQueue::mainQueue();
        for name in unsafe {
            [
                NSWorkspaceDidActivateApplicationNotification,
                NSWorkspaceDidDeactivateApplicationNotification,
                NSWorkspaceDidLaunchApplicationNotification,
                NSWorkspaceDidTerminateApplicationNotification,
                NSWorkspaceDidWakeNotification,
            ]
        } {
            let weak = Rc::downgrade(&listener);
            let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
                contain_callback(|| {
                    let Some(listener) = weak.upgrade() else {
                        return;
                    };
                    let notification = unsafe { notification.as_ref() };
                    if &*notification.name()
                        == unsafe { NSWorkspaceDidTerminateApplicationNotification }
                    {
                        if let Some(app) = notification
                            .userInfo()
                            .and_then(|info| {
                                info.objectForKey(unsafe { NSWorkspaceApplicationKey })
                            })
                            .and_then(|object| object.downcast::<NSRunningApplication>().ok())
                        {
                            listener
                                .processes
                                .borrow_mut()
                                .remove(&app.processIdentifier());
                        }
                    }
                    listener.schedule_refresh();
                });
            });
            // SAFETY: delivery is explicitly on the main queue, so the captured
            // Weak<Rc> never crosses threads; the center copies the block.
            let token = unsafe {
                listener.center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None::<&AnyObject>,
                    Some(&queue),
                    &block,
                )
            };
            listener.tokens.borrow_mut().push(token);
        }
        contain_callback(|| listener.refresh());
        listener
    }

    fn schedule_refresh(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        let block = RcBlock::new(move || {
            contain_callback(|| {
                if let Some(listener) = weak.upgrade() {
                    listener.pending.set(false);
                    listener.refresh();
                }
            })
        });
        if let Some(run_loop) = CFRunLoop::main() {
            // SAFETY: both submission and execution occur on the main run loop.
            // CF copies the escaping block. No timer or repeated polling is used.
            unsafe {
                run_loop.perform_block(
                    kCFRunLoopCommonModes.map(|mode| mode.as_ref()),
                    Some(&block),
                )
            };
            run_loop.wake_up();
        }
    }

    fn refresh(self: &Rc<Self>) {
        let workspace = NSWorkspace::sharedWorkspace();
        let frontmost = workspace.frontmostApplication();
        let mut keep = HashSet::new();
        if let Some(app) = &frontmost {
            keep.insert(app.processIdentifier());
        }
        for app in workspace.runningApplications() {
            if app
                .bundleIdentifier()
                .is_some_and(|id| id.to_string() == "com.apple.appkit.xpc.openAndSavePanelService")
            {
                keep.insert(app.processIdentifier());
            }
        }
        let system = unsafe { AXUIElement::new_system_wide() };
        unsafe { system.set_messaging_timeout(0.15) };
        let focused = attribute::<AXUIElement>(&system, "AXFocusedUIElement");
        if let Some(element) = &focused {
            let mut pid = 0;
            if unsafe { element.pid(NonNull::from(&mut pid)) } == AXError::Success && pid > 0 {
                keep.insert(pid);
            }
        }
        {
            let mut processes = self.processes.borrow_mut();
            processes.retain(|pid, _| keep.contains(pid));
            for pid in keep {
                if let std::collections::hash_map::Entry::Vacant(entry) = processes.entry(pid) {
                    if let Some(observer) = ProcessObserver::new(pid, self) {
                        entry.insert(observer);
                    }
                }
            }
        }
        let mut active = focused.as_deref().is_some_and(inside_picker);
        if !active {
            if let Some(frontmost) = frontmost {
                let app = unsafe { AXUIElement::new_application(frontmost.processIdentifier()) };
                unsafe { app.set_messaging_timeout(0.15) };
                active = attribute::<AXUIElement>(&app, "AXFocusedWindow")
                    .as_deref()
                    .is_some_and(has_picker_sheet);
            }
        }
        (self.changed)(active);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        for token in self.tokens.get_mut().drain(..) {
            unsafe {
                self.center
                    .removeObserver(AsRef::<AnyObject>::as_ref(&*token))
            };
        }
        self.processes.get_mut().clear();
        contain_callback(|| (self.changed)(false));
    }
}

fn contain_callback(callback: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(callback)).is_err() {
        // A custom logging subscriber can panic too; contain that second failure.
        let _ = catch_unwind(|| {
            log::error!("File picker callback panicked; contained at the native boundary");
        });
    }
}

unsafe extern "C-unwind" fn accessibility_changed(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    context: *mut c_void,
) {
    // Destruction notifications carry an invalid element: never query it.
    contain_callback(|| {
        let pointer = context.cast::<Listener>();
        if pointer.is_null() {
            return;
        }
        // SAFETY: observers are owned by this Listener and callbacks execute on
        // its thread. Hold a strong reference until this callback finishes.
        let listener = unsafe {
            Rc::increment_strong_count(pointer);
            Rc::from_raw(pointer)
        };
        listener.schedule_refresh();
    });
}
