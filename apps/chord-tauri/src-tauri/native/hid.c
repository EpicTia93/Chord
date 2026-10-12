// Caps Lock press/release listener.
//
// Caps Lock cannot be observed reliably through a CGEventTap: macOS applies a
// hold delay before toggling the modifier, only reports the toggle (not the
// key-up), and never reports quick taps. The raw HID report (keyboard page,
// usage 0x39) is the only source that behaves like an ordinary key, so this
// file opens every keyboard through IOHIDManager and forwards that usage.
//
// The listener is stoppable and restartable: it runs the run loop in short
// slices and returns as soon as `*stop_flag` becomes non-zero, and every state
// change is reported to the caller instead of being printed to stderr (which
// is lost once the launching terminal closes).

#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/hid/IOHIDManager.h>
#include <string.h>

#include "hid.h"

typedef struct {
    const chord_caps_listener_callbacks *callbacks;
    CFMutableSetRef opened_devices;
} listener_state;

static void copy_product_name(IOHIDDeviceRef device, char *buffer, size_t size) {
    buffer[0] = '\0';
    CFTypeRef product = IOHIDDeviceGetProperty(device, CFSTR(kIOHIDProductKey));
    if (product && CFGetTypeID(product) == CFStringGetTypeID()) {
        if (CFStringGetCString((CFStringRef)product, buffer, (CFIndex)size, kCFStringEncodingUTF8)) {
            return;
        }
    }
    strncpy(buffer, "unknown keyboard", size - 1);
    buffer[size - 1] = '\0';
}

static void report_device(listener_state *state, int kind, IOReturn result, IOHIDDeviceRef device) {
    char product[128];
    copy_product_name(device, product, sizeof(product));
    state->callbacks->on_device(
        state->callbacks->context,
        kind,
        (int32_t)result,
        product,
        (int)CFSetGetCount(state->opened_devices)
    );
}

static void device_matched(void *context, IOReturn result, void *sender, IOHIDDeviceRef device) {
    (void)sender;
    listener_state *state = context;
    if (result != kIOReturnSuccess) {
        // Karabiner-Elements seizes physical keyboards (exclusive access) and
        // exposes a virtual keyboard instead. Chord reads Caps Lock from any
        // device that does open, so a failure here is informational.
        report_device(state, CHORD_CAPS_DEVICE_OPEN_FAILED, result, device);
        return;
    }
    CFSetAddValue(state->opened_devices, device);
    report_device(state, CHORD_CAPS_DEVICE_OPENED, result, device);
}

static void device_removed(void *context, IOReturn result, void *sender, IOHIDDeviceRef device) {
    (void)sender;
    listener_state *state = context;
    if (!CFSetContainsValue(state->opened_devices, device)) {
        return;
    }
    CFSetRemoveValue(state->opened_devices, device);
    report_device(state, CHORD_CAPS_DEVICE_REMOVED, result, device);
}

static void input_callback(void *context, IOReturn result, void *sender, IOHIDValueRef value) {
    (void)sender;
    listener_state *state = context;

    if (result != kIOReturnSuccess || !value) return;

    IOHIDElementRef element = IOHIDValueGetElement(value);
    if (IOHIDElementGetUsagePage(element) != kHIDPage_KeyboardOrKeypad) return;
    if (IOHIDElementGetUsage(element) != kHIDUsage_KeyboardCapsLock) return;

    state->callbacks->on_caps(state->callbacks->context, IOHIDValueGetIntegerValue(value) != 0);
}

static CFMutableDictionaryRef create_keyboard_matching(void) {
    CFMutableDictionaryRef matching = CFDictionaryCreateMutable(
        kCFAllocatorDefault,
        0,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks
    );
    int page = kHIDPage_GenericDesktop;
    int usage = kHIDUsage_GD_Keyboard;
    CFNumberRef page_ref = CFNumberCreate(kCFAllocatorDefault, kCFNumberIntType, &page);
    CFNumberRef usage_ref = CFNumberCreate(kCFAllocatorDefault, kCFNumberIntType, &usage);

    if (!matching || !page_ref || !usage_ref) {
        if (matching) CFRelease(matching);
        if (page_ref) CFRelease(page_ref);
        if (usage_ref) CFRelease(usage_ref);
        return NULL;
    }

    CFDictionarySetValue(matching, CFSTR(kIOHIDDeviceUsagePageKey), page_ref);
    CFDictionarySetValue(matching, CFSTR(kIOHIDDeviceUsageKey), usage_ref);
    CFRelease(page_ref);
    CFRelease(usage_ref);
    return matching;
}

int32_t chord_caps_listener_run(
    const chord_caps_listener_callbacks *callbacks,
    const volatile int *stop_flag
) {
    IOHIDManagerRef manager = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
    if (!manager) return kIOReturnNoMemory;

    CFMutableDictionaryRef matching = create_keyboard_matching();
    if (!matching) {
        CFRelease(manager);
        return kIOReturnNoMemory;
    }
    IOHIDManagerSetDeviceMatching(manager, matching);
    CFRelease(matching);

    listener_state state = {
        .callbacks = callbacks,
        .opened_devices = CFSetCreateMutable(kCFAllocatorDefault, 0, &kCFTypeSetCallBacks),
    };
    if (!state.opened_devices) {
        CFRelease(manager);
        return kIOReturnNoMemory;
    }

    IOHIDManagerRegisterDeviceMatchingCallback(manager, device_matched, &state);
    IOHIDManagerRegisterDeviceRemovalCallback(manager, device_removed, &state);
    IOHIDManagerRegisterInputValueCallback(manager, input_callback, &state);
    IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);

    IOReturn open_result = IOHIDManagerOpen(manager, kIOHIDOptionsTypeNone);
    callbacks->on_manager_opened(callbacks->context, (int32_t)open_result);

    // The manager reports one device's failure even when other keyboards
    // opened, and reports "no device" when keyboards are momentarily absent
    // (Karabiner recreating its virtual keyboard, sleep/wake). Readiness is
    // tracked per device through the callbacks, including hot-plug.
    int32_t result;
    if (open_result == kIOReturnSuccess
        || open_result == kIOReturnExclusiveAccess
        || open_result == kIOReturnNoDevice) {
        result = kIOReturnSuccess;
        while (!*stop_flag) {
            CFRunLoopRunResult run = CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, false);
            if (run == kCFRunLoopRunFinished) {
                // No sources left: the manager has been torn out from under us.
                result = kIOReturnNotOpen;
                break;
            }
        }
    } else {
        result = open_result;
    }

    IOHIDManagerRegisterInputValueCallback(manager, NULL, NULL);
    IOHIDManagerRegisterDeviceMatchingCallback(manager, NULL, NULL);
    IOHIDManagerRegisterDeviceRemovalCallback(manager, NULL, NULL);
    IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    IOHIDManagerClose(manager, kIOHIDOptionsTypeNone);
    CFRelease(manager);
    CFRelease(state.opened_devices);
    return result;
}
