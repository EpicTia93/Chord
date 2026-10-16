//! Main-thread native fixture using the same Rust listener as the application.
#[cfg(target_os = "macos")]
#[path = "../../src/app/frontmost/file_picker/native.rs"]
mod native;

#[cfg(target_os = "macos")]
mod fixture {
    use super::native;
    use block2::RcBlock;
    use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
    use objc2_app_kit::{
        NSAccessibility, NSAlert, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
        NSOpenPanel, NSSavePanel, NSWindow, NSWindowStyleMask,
    };
    use objc2_application_services::AXIsProcessTrusted;
    use objc2_foundation::{
        NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer,
    };
    use std::{
        cell::{Cell, RefCell},
        ptr::NonNull,
        rc::Rc,
    };

    // One-shot test actions and a deadline; production code has no timer.
    fn after(seconds: f64, action: impl FnOnce() + 'static) {
        let action = RefCell::new(Some(action));
        let block = RcBlock::new(move |_: NonNull<NSTimer>| {
            if let Some(action) = action.borrow_mut().take() {
                action();
            }
        });
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_repeats_block(seconds, false, &block);
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    struct Fixture {
        main_thread: MainThreadMarker,
        document: Retained<NSWindow>,
        other: Retained<NSWindow>,
        panel: RefCell<Option<Retained<NSSavePanel>>>,
        listener: RefCell<Option<Rc<native::Listener>>>,
        phase: Cell<u32>,
        active: Cell<bool>,
    }

    impl Fixture {
        fn enable(self: &Rc<Self>) {
            let weak = Rc::downgrade(self);
            let listener = native::Listener::start(self.main_thread, move |active| {
                if let Some(fixture) = weak.upgrade() {
                    fixture.changed(active);
                }
            });
            *self.listener.borrow_mut() = Some(listener);
        }

        fn disable(&self) {
            drop(self.listener.borrow_mut().take());
        }

        fn open_sheet(self: &Rc<Self>) {
            let open = NSOpenPanel::openPanel(self.main_thread);
            open.setTitle(Some(&NSString::from_str("Select a fixture file")));
            let weak = Rc::downgrade(self);
            open.beginSheetModalForWindow_completionHandler(
                &self.document,
                &RcBlock::new(move |_| {
                    if weak
                        .upgrade()
                        .is_some_and(|fixture| fixture.phase.get() == 8)
                    {
                        println!("file-picker-listener-ok");
                        std::process::exit(0);
                    }
                }),
            );
            // NSOpenPanel is a subclass of NSSavePanel.
            *self.panel.borrow_mut() = Some(unsafe { Retained::cast_unchecked(open) });
        }

        fn cancel(&self) {
            if let Some(panel) = self.panel.borrow().as_ref() {
                unsafe { panel.cancel(None) };
            }
        }

        fn changed(self: &Rc<Self>, active: bool) {
            if self.active.replace(active) == active {
                return;
            }
            println!("phase {}: filePickerActive={active}", self.phase.get());
            let fixture = self.clone();
            match (self.phase.get(), active) {
                (0, true) => panic!("An ordinary document or alert activated Finder chords"),
                (1, true) => {
                    self.phase.set(2);
                    after(0.0, move || fixture.other.makeKeyAndOrderFront(None));
                }
                (2, false) => {
                    self.phase.set(3);
                    after(0.0, move || {
                        fixture
                            .panel
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .makeKeyAndOrderFront(None)
                    });
                }
                (3, true) => {
                    self.phase.set(4);
                    after(0.0, move || fixture.cancel());
                }
                (4, false) => {
                    self.phase.set(5);
                    after(0.0, move || {
                        let panel = NSSavePanel::savePanel(fixture.main_thread);
                        panel.setTitle(Some(&NSString::from_str("Fixture with a custom title")));
                        panel.beginWithCompletionHandler(&RcBlock::new(|_| {}));
                        *fixture.panel.borrow_mut() = Some(panel);
                    });
                }
                (5, true) => {
                    self.phase.set(6);
                    after(0.0, move || fixture.cancel());
                }
                (6, false) => {
                    self.phase.set(7);
                    after(0.0, move || {
                        fixture.disable();
                        fixture.document.makeKeyAndOrderFront(None);
                        fixture.open_sheet();
                        after(0.5, move || fixture.enable());
                    });
                }
                (7, true) => {
                    self.phase.set(8);
                    after(0.0, move || {
                        fixture.disable();
                        assert!(
                            !fixture.active.get(),
                            "Disabling left picker context active"
                        );
                        fixture.cancel();
                    });
                }
                _ => {}
            }
        }
    }

    fn window(main_thread: MainThreadMarker, x: f64, title: &str) -> Retained<NSWindow> {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(main_thread),
                NSRect::new(NSPoint::new(x, 100.0), NSSize::new(600.0, 400.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str(title));
        window
    }

    pub fn run() {
        let main_thread = MainThreadMarker::new().expect("fixture must run on the main thread");
        let app = NSApplication::sharedApplication(main_thread);
        assert_eq!(
            NSOpenPanel::openPanel(main_thread)
                .accessibilityIdentifier()
                .map(|s| s.to_string())
                .as_deref(),
            Some("open-panel")
        );
        assert_eq!(
            NSSavePanel::savePanel(main_thread)
                .accessibilityIdentifier()
                .map(|s| s.to_string())
                .as_deref(),
            Some("save-panel")
        );
        println!("file-picker-identifiers-ok");
        if !std::env::args().any(|arg| arg == "--ui") {
            return;
        }
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("UI test requires Accessibility access for the fixture executable");
            std::process::exit(77);
        }
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();
        let fixture = Rc::new(Fixture {
            main_thread,
            document: window(main_thread, 100.0, "Chord file picker fixture"),
            other: window(main_thread, 750.0, "Ordinary document"),
            panel: RefCell::new(None),
            listener: RefCell::new(None),
            phase: Cell::new(0),
            active: Cell::new(false),
        });
        fixture.document.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        let setup = fixture.clone();
        after(0.0, move || {
            setup.enable();
            assert!(!setup.active.get());
            let alert = NSAlert::new(main_thread);
            alert.setMessageText(&NSString::from_str(
                "An ordinary sheet is not a file picker",
            ));
            let opened = setup.clone();
            alert.beginSheetModalForWindow_completionHandler(
                &setup.document,
                Some(&RcBlock::new(move |_| {
                    opened.phase.set(1);
                    opened.open_sheet();
                })),
            );
            after(0.5, move || {
                assert!(
                    !setup.active.get(),
                    "An unrelated alert activated Finder chords"
                );
                setup.document.endSheet(&alert.window());
                alert.window().orderOut(None);
            });
        });
        after(20.0, move || {
            eprintln!("Timed out at phase {}", fixture.phase.get());
            fixture.disable();
            std::process::exit(1);
        });
        app.run();
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    fixture::run();
}
