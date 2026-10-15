// Compile with the production native/file_picker.m. UI mode uses a controlled
// AppKit fixture and event callbacks, with one deadline instead of polling.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import "../../native/file_picker.h"

static NSWindow *document;
static NSWindow *otherWindow;
static NSSavePanel *panel;
static unsigned phase;
static bool active;

static void dumpElement(AXUIElementRef element, unsigned depth) {
    if (!element || depth > 3) return;
    for (NSString *name in @[(__bridge NSString *)kAXRoleAttribute, (__bridge NSString *)kAXSubroleAttribute,
                             (__bridge NSString *)kAXIdentifierAttribute, (__bridge NSString *)kAXTitleAttribute]) {
        CFStringRef key = (__bridge CFStringRef)name;
        CFTypeRef value = NULL;
        if (AXUIElementCopyAttributeValue(element, key, &value) == kAXErrorSuccess && value) {
            CFShow(key);
            CFShow(value);
            CFRelease(value);
        }
    }
    CFTypeRef children = NULL;
    if (AXUIElementCopyAttributeValue(element, kAXChildrenAttribute, &children) == kAXErrorSuccess && children) {
        if (CFGetTypeID(children) == CFArrayGetTypeID()) {
            for (CFIndex index = 0; index < MIN(CFArrayGetCount(children), 15); index++)
                dumpElement((AXUIElementRef)CFArrayGetValueAtIndex(children, index), depth + 1);
        }
        CFRelease(children);
    }
}

static void finish(int code, const char *message) {
    fprintf(code ? stderr : stdout, "%s\n", message);
    chord_file_picker_set_enabled(false, NULL);
    exit(code);
}

static void later(dispatch_block_t action) {
    dispatch_async(dispatch_get_main_queue(), action);
}

static void changed(bool value) {
    bool previous = active;
    active = value;
    if (previous == value) return;
    fprintf(stdout, "phase %u: filePickerActive=%s\n", phase, value ? "true" : "false");
    fflush(stdout);
    if (phase == 1 && value) {
        phase = 2;
        later(^{ [otherWindow makeKeyAndOrderFront:nil]; });
    } else if (phase == 2 && !value) {
        phase = 3;
        later(^{ [panel makeKeyAndOrderFront:nil]; });
    } else if (phase == 3 && value) {
        phase = 4;
        later(^{ [panel cancel:nil]; });
    } else if (phase == 4 && !value) {
        phase = 5;
        later(^{
            panel = NSSavePanel.savePanel;
            panel.title = @"Fixture with a custom title";
            [panel beginWithCompletionHandler:^(NSModalResponse result) {}];
        });
    } else if (phase == 5 && value) {
        phase = 6;
        later(^{ [panel cancel:nil]; });
    } else if (phase == 6 && !value) {
        phase = 7;
        later(^{
            chord_file_picker_set_enabled(false, changed);
            [document makeKeyAndOrderFront:nil];
            panel = NSOpenPanel.openPanel;
            [panel beginSheetModalForWindow:document completionHandler:^(NSModalResponse result) {
                if (phase == 8) finish(0, "file-picker-listener-ok");
            }];
            // An already-open panel must be detected by the initial snapshot.
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                chord_file_picker_set_enabled(true, changed);
            });
        });
    } else if (phase == 7 && value) {
        phase = 8;
        later(^{
            chord_file_picker_set_enabled(false, changed);
            if (active) finish(1, "Disabling the listener left picker context active");
            [panel cancel:nil];
        });
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        // These are native AppKit identifiers, independent of localized titles.
        if (![NSOpenPanel.openPanel.accessibilityIdentifier isEqualToString:@"open-panel"]
            || ![NSSavePanel.savePanel.accessibilityIdentifier isEqualToString:@"save-panel"])
            finish(1, "Unexpected native Open/Save panel identifiers");
        if (argc > 1 && strcmp(argv[1], "--identifiers") == 0) {
            puts("file-picker-identifiers-ok");
            return 0;
        }
        if (!AXIsProcessTrusted()) {
            fputs("UI test requires Accessibility access for the fixture executable.\n", stderr);
            return 77;
        }
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        [NSApp finishLaunching];
        document = [[NSWindow alloc] initWithContentRect:NSMakeRect(100, 100, 600, 400)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        document.title = @"Chord file picker fixture";
        otherWindow = [[NSWindow alloc] initWithContentRect:NSMakeRect(750, 100, 300, 200)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        otherWindow.title = @"Ordinary document";
        [document makeKeyAndOrderFront:nil];
        [NSApp activateIgnoringOtherApps:YES];
        later(^{
            chord_file_picker_set_enabled(true, changed);
            if (active) finish(1, "An ordinary document activated Finder chords");
            NSAlert *alert = [NSAlert new];
            alert.messageText = @"An ordinary sheet is not a file picker";
            [alert beginSheetModalForWindow:document completionHandler:^(NSModalResponse result) {
                phase = 1;
                panel = NSOpenPanel.openPanel;
                panel.title = @"Select a fixture file";
                [panel beginSheetModalForWindow:document completionHandler:^(NSModalResponse result) {}];
            }];
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 500 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                if (active) finish(1, "An unrelated alert activated Finder chords");
                [NSApp endSheet:alert.window];
                [alert.window orderOut:nil];
            });
        });
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 20 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            fprintf(stderr, "Timed out at phase %u\n", phase);
            NSLog(@"Frontmost: %@; fixture active: %d pid: %d", NSWorkspace.sharedWorkspace.frontmostApplication, NSApp.isActive, getpid());
            AXUIElementRef app = AXUIElementCreateApplication(getpid());
            AXUIElementRef focused = NULL;
            AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute, (CFTypeRef *)&focused);
            dumpElement(focused, 0);
            if (focused) CFRelease(focused);
            CFRelease(app);
            finish(1, "File picker lifecycle notification was not delivered");
        });
        [NSApp run];
    }
}
