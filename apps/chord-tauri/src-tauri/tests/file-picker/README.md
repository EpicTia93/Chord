# Native file-picker listener

From the repository root, compile the production listener and check AppKit's
nonlocalized Open/Save identifiers:

```sh
bun apps/chord-tauri/src-tauri/tests/file-picker/verify.ts
```

Add `--ui` to exercise real native panels in a controlled AppKit fixture. The
executable needs Accessibility access and an active desktop session. It returns
77 when permission is unavailable; it never prompts or changes permissions.

```sh
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer bun apps/chord-tauri/src-tauri/tests/file-picker/verify.ts --ui
```

Use a matching Xcode compiler and SDK. The explicit `DEVELOPER_DIR` above also
avoids mismatched Command Line Tools installations.

The fixture verifies an unrelated alert remains inactive, a native Open sheet
activates the listener, switching to another document clears it, returning to the
sheet reactivates it, cancellation clears it, a standalone Save panel works with
a custom title, attaching to an already-open picker works, and disabling releases
the listener and clears its state. Assertions follow Accessibility callbacks;
there is a single overall timeout and no polling loop. Every panel is canceled.

Rust's `file_picker_tests` cover scope priority, fallback, deduplication,
preservation of the actual foreground app, and the protected core namespace.
