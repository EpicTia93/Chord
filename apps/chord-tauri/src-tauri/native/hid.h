#ifndef CHORD_HID_H
#define CHORD_HID_H

#include <stdint.h>

// Kinds reported through `on_device`.
#define CHORD_CAPS_DEVICE_OPENED 0
#define CHORD_CAPS_DEVICE_OPEN_FAILED 1
#define CHORD_CAPS_DEVICE_REMOVED 2

typedef struct {
    void *context;
    // `pressed` is 1 on key-down and 0 on key-up.
    void (*on_caps)(void *context, int pressed);
    // `open_count` is the number of keyboards currently open after this event.
    void (*on_device)(void *context, int kind, int32_t io_return, const char *product, int open_count);
    void (*on_manager_opened)(void *context, int32_t io_return);
} chord_caps_listener_callbacks;

// Blocks until `*stop_flag` becomes non-zero. Returns kIOReturnSuccess (0)
// after a requested stop, otherwise the IOReturn that prevented listening.
int32_t chord_caps_listener_run(
    const chord_caps_listener_callbacks *callbacks,
    const volatile int *stop_flag
);

#endif
