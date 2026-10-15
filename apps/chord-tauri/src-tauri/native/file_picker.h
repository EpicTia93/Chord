#pragma once
#include <stdbool.h>

// Main-thread only. Disabling releases all observers and publishes false.
void chord_file_picker_set_enabled(bool enabled, void (*changed)(bool active));
