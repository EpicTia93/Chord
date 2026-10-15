#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import "file_picker.h"

static CFTypeRef copyAttribute(AXUIElementRef element, CFStringRef attribute) {
    CFTypeRef value = NULL;
    if (AXUIElementCopyAttributeValue(element, attribute, &value) != kAXErrorSuccess) {
        if (value) CFRelease(value);
        return NULL;
    }
    return value;
}

static AXUIElementRef copyElement(AXUIElementRef element, CFStringRef attribute) {
    CFTypeRef value = copyAttribute(element, attribute);
    if (value && CFGetTypeID(value) == AXUIElementGetTypeID()) return (AXUIElementRef)value;
    if (value) CFRelease(value);
    return NULL;
}

static bool isPicker(AXUIElementRef element) {
    CFTypeRef identifier = copyAttribute(element, kAXIdentifierAttribute);
    // AppKit uses these nonlocalized identifiers for both windows and sheets.
    // AXDialog/AXSheet alone also match alerts and other unrelated dialogs.
    bool result = identifier && CFGetTypeID(identifier) == CFStringGetTypeID()
        && (CFEqual(identifier, CFSTR("open-panel")) || CFEqual(identifier, CFSTR("save-panel")));
    if (identifier) CFRelease(identifier);
    return result;
}

static bool isInsidePicker(AXUIElementRef element) {
    AXUIElementRef current = (AXUIElementRef)CFRetain(element);
    bool result = false;
    for (unsigned depth = 0; current && depth < 32; depth++) {
        if (isPicker(current)) {
            result = true;
            break;
        }
        AXUIElementRef parent = copyElement(current, kAXParentAttribute);
        CFRelease(current);
        current = parent;
    }
    if (current) CFRelease(current);
    return result;
}

static bool hasPickerSheet(AXUIElementRef window) {
    if (isPicker(window)) return true;
    CFTypeRef sheets = copyAttribute(window, kAXChildrenAttribute);
    bool result = false;
    if (sheets && CFGetTypeID(sheets) == CFArrayGetTypeID()) {
        CFArrayRef array = (CFArrayRef)sheets;
        for (CFIndex index = 0; index < CFArrayGetCount(array); index++) {
            CFTypeRef sheet = CFArrayGetValueAtIndex(array, index);
            if (sheet && CFGetTypeID(sheet) == AXUIElementGetTypeID() && isPicker((AXUIElementRef)sheet)) {
                result = true;
                break;
            }
        }
    }
    if (sheets) CFRelease(sheets);
    return result;
}

@class ChordFilePickerListener;
static void accessibilityChanged(AXObserverRef observer, AXUIElementRef element,
                                 CFStringRef notification, void *context);

@interface ChordPickerProcess : NSObject
@property(nonatomic, readonly) pid_t pid;
- (instancetype)initWithPID:(pid_t)pid listener:(ChordFilePickerListener *)listener;
@end

@interface ChordFilePickerListener : NSObject
- (instancetype)initWithCallback:(void (*)(bool))callback;
- (void)scheduleRefresh;
@end

@implementation ChordPickerProcess {
    AXObserverRef _observer;
    AXUIElementRef _application;
}

- (instancetype)initWithPID:(pid_t)pid listener:(ChordFilePickerListener *)listener {
    if (!(self = [super init])) return nil;
    _pid = pid;
    _application = AXUIElementCreateApplication(pid);
    if (!_application) return nil;
    AXUIElementSetMessagingTimeout(_application, 0.15);
    if (AXObserverCreate(pid, accessibilityChanged, &_observer) != kAXErrorSuccess) return nil;
    CFStringRef notifications[] = {
        kAXWindowCreatedNotification, kAXSheetCreatedNotification,
        kAXFocusedWindowChangedNotification, kAXFocusedUIElementChangedNotification,
        kAXUIElementDestroyedNotification,
    };
    unsigned subscribed = 0;
    for (unsigned index = 0; index < sizeof(notifications) / sizeof(notifications[0]); index++) {
        AXError error = AXObserverAddNotification(_observer, _application, notifications[index],
                                                 (__bridge void *)listener);
        if (error == kAXErrorSuccess) subscribed++;
        else if (error != kAXErrorNotificationUnsupported)
            NSLog(@"Chord file picker: subscription %@ for pid %d failed (%d)", notifications[index], pid, error);
    }
    if (!subscribed) return nil;
    CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(_observer), kCFRunLoopCommonModes);
    return self;
}

- (void)dealloc {
    if (_observer) {
        CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(_observer), kCFRunLoopCommonModes);
        CFRelease(_observer);
    }
    if (_application) CFRelease(_application);
}
@end

@implementation ChordFilePickerListener {
    NSMutableDictionary<NSNumber *, ChordPickerProcess *> *_processes;
    NSMutableArray<id> *_workspaceObservers;
    void (*_changed)(bool);
    bool _refreshPending;
}

- (instancetype)initWithCallback:(void (*)(bool))callback {
    if (!(self = [super init])) return nil;
    _changed = callback;
    _processes = [NSMutableDictionary new];
    _workspaceObservers = [NSMutableArray new];
    NSNotificationCenter *center = NSWorkspace.sharedWorkspace.notificationCenter;
    __weak ChordFilePickerListener *weakSelf = self;
    for (NSNotificationName name in @[
        NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidDeactivateApplicationNotification,
        NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
        NSWorkspaceDidWakeNotification,
    ]) {
        id token = [center addObserverForName:name object:nil queue:NSOperationQueue.mainQueue
            usingBlock:^(NSNotification *notification) {
                ChordFilePickerListener *listener = weakSelf;
                if (!listener) return;
                if ([notification.name isEqualToString:NSWorkspaceDidTerminateApplicationNotification]) {
                    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
                    [listener->_processes removeObjectForKey:@(app.processIdentifier)];
                }
                [listener scheduleRefresh];
            }];
        [_workspaceObservers addObject:token];
    }
    // Subscribe before inspecting so an already-open panel is covered too.
    [self refresh];
    return self;
}

- (void)observePID:(pid_t)pid {
    if (pid <= 0 || _processes[@(pid)]) return;
    ChordPickerProcess *process = [[ChordPickerProcess alloc] initWithPID:pid listener:self];
    if (process) _processes[@(pid)] = process;
}

- (void)scheduleRefresh {
    if (_refreshPending) return;
    _refreshPending = true;
    __weak ChordFilePickerListener *weakSelf = self;
    // Coalesce notification bursts on the run loop; there is no polling timer.
    dispatch_async(dispatch_get_main_queue(), ^{
        ChordFilePickerListener *listener = weakSelf;
        if (!listener) return;
        listener->_refreshPending = false;
        [listener refresh];
    });
}

- (void)refresh {
    NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
    NSRunningApplication *frontmost = workspace.frontmostApplication;
    NSMutableSet<NSNumber *> *keep = [NSMutableSet new];
    if (frontmost) [keep addObject:@(frontmost.processIdentifier)];
    // Open/Save content is hosted out of process on modern macOS. Subscribe to
    // existing and newly launched hosts, not only the app that opened the panel.
    for (NSRunningApplication *app in workspace.runningApplications) {
        if ([app.bundleIdentifier isEqualToString:@"com.apple.appkit.xpc.openAndSavePanelService"])
            [keep addObject:@(app.processIdentifier)];
    }
    AXUIElementRef system = AXUIElementCreateSystemWide();
    AXUIElementSetMessagingTimeout(system, 0.15);
    AXUIElementRef focused = copyElement(system, kAXFocusedUIElementAttribute);
    if (focused) {
        pid_t pid = 0;
        if (AXUIElementGetPid(focused, &pid) == kAXErrorSuccess && pid > 0) [keep addObject:@(pid)];
    }
    for (NSNumber *pid in _processes.allKeys) {
        if (![keep containsObject:pid]) [_processes removeObjectForKey:pid];
    }
    for (NSNumber *pid in keep) [self observePID:pid.intValue];

    bool active = focused && isInsidePicker(focused);
    if (!active && frontmost) {
        AXUIElementRef app = AXUIElementCreateApplication(frontmost.processIdentifier);
        AXUIElementSetMessagingTimeout(app, 0.15);
        AXUIElementRef window = copyElement(app, kAXFocusedWindowAttribute);
        // Attached panels can be exposed as a sheet of the focused document
        // window even when the system-wide focused-element query is unavailable.
        active = window && hasPickerSheet(window);
        if (window) CFRelease(window);
        CFRelease(app);
    }
    if (focused) CFRelease(focused);
    CFRelease(system);
    // Rust deduplicates publications. Always report this snapshot: an app
    // activation may have invalidated its cache even if both apps have a picker.
    _changed(active);
}

- (void)dealloc {
    NSNotificationCenter *center = NSWorkspace.sharedWorkspace.notificationCenter;
    for (id observer in _workspaceObservers) [center removeObserver:observer];
    // Release AX observers before their unretained callback context disappears.
    [_processes removeAllObjects];
}
@end

static void accessibilityChanged(AXObserverRef observer, AXUIElementRef element,
                                 CFStringRef notification, void *context) {
    (void)observer;
    (void)element;
    (void)notification;
    // A destruction notification's element is invalid: never query it here.
    [(__bridge ChordFilePickerListener *)context scheduleRefresh];
}

void chord_file_picker_set_enabled(bool enabled, void (*changed)(bool)) {
    static ChordFilePickerListener *listener;
    NSCAssert(NSThread.isMainThread, @"File picker observers require the main thread");
    if (enabled && !listener) listener = [[ChordFilePickerListener alloc] initWithCallback:changed];
    if (!enabled) {
        listener = nil;
        if (changed) changed(false);
    }
}
