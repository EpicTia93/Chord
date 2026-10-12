# JavaScript

## Recommended Setup

TODO

## Recommended Packages

- [bplist-lossless](https://github.com/leonsilicon/bplist-lossless) - A binary plist parser specifically tailored for edits by avoiding loss of precision during parsing and re-serialization.
- [doctor-json](https://github.com/privatenumber/doctor-json) - A JSON editor that preserves all existing formatting/comments
- [keycode-ts2](https://github.com/leonsilicon/keycode-ts2) - A TypeScript port of the [Rust `keycode` crate](https://crates.io/crates/keycode) which uses the Chromium keycode names as the source of truth (_Chords_ uses these keycode names as the source of truth).

## Chord events and persistent return values

Chord actions use `@trigger:event`, with an optional `=identifier` to retain the handler's return value:

```toml
[on.prepare]
file = "prepare.js"

[on.update]
file = "update.js"

[chords.'j,(.*)']
name = "Live clicks"
'@init:prepare=context' = []
'@keydown:update=latest' = ['$context', '$1']
```

The event name refers to an entry in `[on]`. Every event specifier accepts an argument array and may assign a return value. Identifiers contain ASCII letters, digits, or underscores and start with a letter or underscore. An exact argument such as `$context` passes the stored JavaScript value; `$1`, `$2`, etc. continue to pass regex captures. Named references also work inside argument arrays and tables. Referencing an identifier before it has been assigned raises a `ReferenceError`.

Chord keys are literal except for parenthesized regex sections. For example, `hs.(\d+)` matches `hs.12` and captures `12` as `$1`; the dot outside parentheses needs no backslash. Regex syntax, including escaped dots, nested groups, alternatives, and quantifiers, belongs inside the parentheses. Patterns match the entire entered sequence automatically, so no outer `^` or `$` is needed. The same rule applies to regex help hints; compact help ranges retain their special display behavior.

Handlers receive the entered sequence as `this.input` (for example, `hsr12`), alongside `this.focusedAppId` and `this.event`. This also works for CLI execution.

| Trigger | When it calls the handler |
| --- | --- |
| `@init:event` | Once when this chord starts matching, before its other event handlers. |
| `@keydown:event` | After each keydown, using the buffer after any edit. |
| `@keypress:event` | After the matching keydown handlers, for every key press, including Backspace and modifiers. |
| `@keyup:event` | After each key release, using the current buffer. |
| `@input:event` | When the chord is completed with Caps Lock, the final shifted key, Space release, or the CLI. |

All matching lifecycle chords are visited in app/global and package priority order. Ordinary input completion keeps its first-match action priority. Initializers run before the other callbacks for a buffer snapshot, and callbacks are awaited in event order, including Promise return values. A callback failure stops the remaining callbacks for that snapshot and is reported through Chord's normal handler-error path.

Values belong to that chord's current matching interval. When the buffer stops matching, clears, or Chord mode ends, its initialization state and values are discarded. Matching again runs init again with fresh values. Different chords and focused apps have separate values. Returned objects, functions, and `undefined` retain their JavaScript identity. A later assignment to the same identifier replaces its value.

For example, `prepare.js` can return a state object and `update.js` can reuse it:

```js
// prepare.js
export default function buildHandler() {
  return async function prepare() {
    return { updates: 0 };
  };
}

// update.js
export default function buildHandler() {
  return function update(context, sequence) {
    if (this.event.key === "Backspace") return context.updates;
    context.updates += 1;
    return context.updates;
  };
}
```

`this.event` follows the [Web KeyboardEvent data model](https://www.w3.org/TR/uievents/#interface-keyboardevent): `type`, `key`, `code`, `location`, `ctrlKey`, `shiftKey`, `altKey`, `metaKey`, `repeat`, `isComposing`, and `timeStamp`. `getModifierState()` supports `Alt`, `Control`, `Meta`, and `Shift`. Non-character keys use names such as `Backspace`, `Escape`, and `Shift`; `code` identifies the physical key, such as `KeyJ` or `ShiftLeft`. Character names use Chord's existing key mapping. Repeated keydowns are delivered with `repeat: true` without adding duplicate buffer characters.

The snapshot extends Bun's `Event`, with read-only keyboard fields, `view: null`, and `detail: 0`. It has no DOM target or propagation path and is non-cancelable, so `preventDefault()` does not change native buffer handling. Composition events are not captured (`isComposing` is false). Init and input callbacks receive the originating keyboard event; `@keypress` callbacks receive `type: "keypress"`. CLI execution does not synthesize keyboard events and uses `this.event: null`.

Chord deliberately sends `@keypress` for every key, including Backspace, even though the browser's legacy `keypress` event has narrower behavior. Normal buffer edits and completion controls still apply; keyboard callbacks observe the resulting buffer.

## The built-in `chord` module

The built-in `chord` module is intentionally kept minimal.

```ts
export class Applescript {
	constructor(source: string);
	constructor(fn: (...args: unknown[]) => unknown, ...args: unknown[]);
	compile(): void;
	execute(): unknown;
}

export function setAppNeedsRelaunch(bundleId: string, needsRelaunch: boolean): void;

export type WindowBounds = { x: number; y: number; width: number; height: number };
export function getAppWindows(bundleId: string): {
  windowId: number;
  processId: number;
  bounds: WindowBounds;
  title: string | null;
}[];
export function click(options: {
  windowId: number;
  x: number;
  y: number;
  expectedBounds?: WindowBounds;
}): Promise<void>;
```

Chord also attaches a namespace to every module it loads, so path resolution is bound to the
calling module rather than passed in:

```ts
interface ImportMeta {
  chord: {
    resolveNative(relpath: string): string;
    resolvePackageFile(path: string): string;
  };
}
```

This marks or clears an app in the settings UI and gives the user a one-click relaunch button.

`Applescript` uses the macOS `OSAKit` framework via the Rust `osakit` crate. Call `compile()` before `execute()`.
When constructed with a string it runs AppleScript source directly. When constructed with a function, Chord serializes the trailing args, uses `fn.toString()`, wraps it as JXA, and executes it as `JavaScript` in OSAKit.

`getAppWindows` lists an already-running app's on-screen normal windows without launching or
activating it. Windows retain their IDs and geometry even when another app covers them. Titles
can be null when macOS does not expose them. Select the intended window before sending input.

`click` sends a left-button down/up pair to that window. `x` and `y` are window-local macOS
logical screen points measured from its top-left corner; they are independent of Retina scaling.
The helper preserves the Mac pointer and foreground app. Optional `expectedBounds` rejects a
window that has moved or resized since coordinates were calculated. Invalid coordinates, missing
windows, denied Accessibility access, and unavailable private APIs reject the Promise.
These helpers work in both the app and `chord bun` CLI mode; they do not require an app context.

```ts
import { click, getAppWindows } from "chord";

const windows = getAppWindows("com.apple.ScreenContinuity");
if (windows.length !== 1) throw new Error("Select the intended Mirroring window");
const window = windows[0];
await click({
  windowId: window.windowId,
  x: window.bounds.width / 2,
  y: window.bounds.height - 20,
  expectedBounds: window.bounds,
});
```

Rust callers use `chords_lib::native_input::{get_app_windows, click, ClickOptions}`. The click
helper holds the button for 80 ms and should run on a worker thread. Chord's JS bridge does this
automatically. The macOS implementation uses `NSEvent`'s window number, both window-under-pointer
fields, mouse subtype 3, and Command on background events, stamps window-local coordinates through
the private `CGEventSetWindowLocation` symbol, then calls `CGEventPostToPid`. It never posts a mouse
move or activates the target, and does not fall back to cursor-moving input if the SPI is missing.
This private recipe was verified on macOS 26.5.2; its availability and behavior can change with OS updates.

## URL Scheme

Chord registers the `chord:` URL scheme on macOS so scripts and launcher tools can trigger app actions.

### Commands

- `ui`
- `settings`
- `open-settings`
- `show-settings`
- `reload-config`
- `reload-configs`

### Examples

```sh
# Open the Chord UI
open --background 'chord:ui'

# Open the settings window
open --background 'chord:settings'

# Open the settings window (host-style form)
open --background 'chord://settings'

# Reload chord configs
open --background 'chord:reload-config'
```

## CLI

This repo also includes a small `chord` CLI wrapper. It delegates dynamic commands to the compiled
Chord executable rather than putting command payloads in the public URL scheme. Chord sequences
execute in the running app (launching it in the background when needed) so they use its loaded
packages and frontmost-app context; `bun` uses Chord's embedded runtime, and `exec` runs the command
as a Chord child process.

### Commands

- `ui`
- `settings`
- `open-settings`
- `show-settings`
- `reload-config`
- `reload-configs`
- `bun <script> [args...]` (alias: `js`)
- `run <file> <export> [args...]`
- `exec <shell-command>`
- `chord <sequence>`
- `<sequence>` (shorthand for `chord <sequence>`)

### Examples

```sh
./chord bun ./script.ts hello
./chord js ./script.ts hello
./chord run ./script.ts main hello
./chord exec 'open https://example.com'
./chord chord fq
./chord fq
./chord ui
./chord settings
./chord reload-configs
```

If you want to run it as `chord` from anywhere, add the repo copy to your `PATH` or symlink it into a directory that is already on your `PATH`.

The wrapper finds a development/release build in `src-tauri/target`, then checks the standard
`/Applications` locations. Set `CHORD_EXECUTABLE` when the binary lives somewhere else.

The static settings/reload commands depend on macOS recognizing the bundled Chord app as the
handler for the `chord:` URL scheme, so the app bundle needs to be built and launched at least once
first.

# Native code

Every chord handler is a JavaScript module, and Chord runs it on an embedded [Bun](https://bun.sh) runtime via the [`rbun`](https://github.com/KeyChord/rbun) crate. Native Swift is exposed as a Node-API add-on built with [NodeSwift](https://github.com/kabiroberai/node-swift):

`src/swift/menu/menu.swift` → `target/<triple>/menu/menu.node`

`@keychord/config` discovers and compiles this layout automatically. It publishes NodeSwift's dynamic `libNodeAPI.dylib` beside each `.node` file. Keeping that runtime dynamic lets Chord safely load add-ons from several packages in one process; the package does not need an npm dependency on NodeSwift at runtime.

> **Trust:** native code runs inside Chord with your full user permissions. Only enable packages from sources you trust.

## Package layout

```text
my-package/
├── chords/macos.toml
├── src/
│   ├── js/menu.ts                 # resolves and loads the Node-API add-on
│   └── swift/
│       ├── _shared/**             # compiled into every add-on
│       └── menu/
│           ├── menu.swift         # entry point: exports #NodeModule
│           └── helpers.swift      # optional additional Swift source
├── js/menu.js                     # portable JavaScript build output
└── target/                        # platform-specific output — commit/publish it like js/
    └── aarch64-apple-darwin/
        └── menu/
            ├── menu.node          # Node-API add-on
            └── libNodeAPI.dylib   # shared NodeSwift runtime, found through @loader_path
```

`src/` is authored source, `chords/` is Chord configuration, `js/` holds portable JavaScript artifacts, and `target/<triple>/` holds native artifacts. Native build state lives in `node_modules/.cache/chord/native/`, covered by the usual `node_modules/` ignore. Keep `target/` tracked.

## Writing one

Import `NodeAPI` and declare the add-on exports with `#NodeModule`. NodeSwift converts supported Swift values and thrown errors at the Node-API boundary, so no C ABI or manual pointer ownership is needed:

```swift
// src/swift/beep/beep.swift
import AppKit
import NodeAPI

enum BeepError: Error {
    case invalidCount
}

#NodeModule(exports: [
    "beep": try NodeFunction { (times: Int) throws in
        guard times > 0 else { throw BeepError.invalidCount }
        for _ in 0..<times { NSSound.beep() }
        return try NodeUndefined()
    },
])
```

The handler resolves the add-on through `import.meta.chord`—never with a hardcoded target triple—and loads it with Bun's Node-compatible `process.dlopen`:

```ts
// src/js/beep.ts
type BeepAddon = { beep(times: number): void };
let addon: BeepAddon | undefined;

function openAddon(): BeepAddon {
  const module = { exports: {} as BeepAddon };
  process.dlopen(module, import.meta.chord.resolveNative("beep"));
  return module.exports;
}

export default function build(times = 1) {
  return function beep() {
    addon ??= openAddon();
    addon.beep(times);
  };
}
```

- `import.meta.chord.resolveNative("beep")` returns the absolute path of `target/<Chord's triple>/beep/beep.node` for the calling module's package, including when the package is vendored inside another one. `import.meta.chord.resolvePackageFile("any/relative/path")` does the same for arbitrary package files. Both are bound to the module they are read from, so a module can only resolve paths within its own package.
- Each installed package runs in its own supervised process with a persistent embedded Bun VM. Imports, handler factories, lifecycle callbacks, timers, and native add-ons all stay in that process. On macOS the worker services AppKit events and Swift's `@MainActor`; keep NodeAPI values on `@NodeActor` and pass Swift values to the main actor. Return or await native Promises so Chord waits for completion and reports errors.
- JavaScript and Swift exceptions are reported as handler errors. A native abort, bad pointer, or `process.exit()` terminates only the package worker. Chord disables that worker until configuration reload, leaves other packages running, and never replays the failed action. Setup and health checks have a 15-second deadline; actions and lifecycle callbacks have a five-minute deadline for interactive UI.
- Reloading configuration replaces package workers, so rebuilt JavaScript, transitive imports, and native add-ons are loaded into fresh VMs. Return-value assignments preserve JavaScript object identity inside the package's VM between events in the same matching interval.
- Workers retain the user's filesystem and process access; this is crash isolation, not a permissions sandbox.
- Package stdout and stderr inherit Chord's log streams.

## Declaring it

Nothing special: it is a JavaScript handler.

```toml
[on.beep]
file = "beep.js"
args = [2]

[chords."-b"]
"@input:beep" = []
```

The old `kind = "native"` declaration is rejected with a pointer to this section.

## Using native handlers from other packages

Vendor the dependency with `config({ vendor: ["@keychord/chords-menu"] })` and import its JavaScript normally. Vendoring copies the dependency's `target/` tree alongside its `js/` and `chords/`; the dependency's own `import.meta.chord.resolveNative("menu")` call automatically maps to `target/@keychord/chords-menu/target/<triple>/menu/menu.node`. Add-ons are consumed through their JavaScript API, not by importing their generated Swift modules.

For compatibility with installed packages, the `chord` module still exports `resolveNativeModulePath(import.meta, relpath)` and `resolvePackageFile(import.meta, path)`. These delegate to the same per-module resolvers; new handlers should use `import.meta.chord` directly.

## Building

Packages built with `@keychord/config` compile `src/swift` automatically as part of `vp pack` (and rebuild on change in `vp pack --watch`). To build every distributable folder at once—`js/` and `target/<triple>/`—run `vpr compile` (`--triple <triple>` to cross-build, `--skip-js`/`--skip-native` for one half, `vpr -r compile` for the whole workspace). Building needs a Swift toolchain from Xcode or the Command Line Tools. Options go in `vite.config.ts`:

```ts
import { config } from "@keychord/config";

export default config({
  native: {
    triples: ["aarch64-apple-darwin", "x86_64-apple-darwin"], // default: host triple
    targets: {
      menu: {
        frameworks: ["ApplicationServices"], // imports autolink; list extras here
        swiftFlags: [],
        linkerFlags: [],
      },
    },
  },
});
```

`@keychord/config` builds the package's root `Package.swift` directly. SwiftPM build products and staging files live in each package's `node_modules/.cache/chord/native/`, grouped by target triple and add-on; distributable files are copied to `target/`. `@keychord/config` currently supports Apple triples.

Any other build system works too: Chord only cares that `target/<triple>/<path>/<name>.node` is a Node-API add-on compatible with Bun's `process.dlopen` and that its dynamic dependencies are resolvable. For the standard NodeSwift build, `libNodeAPI.dylib` sits beside the add-on and is found through `@loader_path`.

## Testing without the app

A Chord build's CLI runs a script on the same embedded Bun, with the `chord` module available (`import.meta.chord.resolveNative` anchors on the nearest `package.json`):

```sh
chord bun scripts/run.ts by-letters f     # from the chords-menu checkout
```

## Troubleshooting

- `process.dlopen` failures name the missing file or Node-API registration problem; check that the add-on and `libNodeAPI.dylib` under `target/<your triple>/` were built and committed and that the `#NodeModule` export names match the TypeScript binding.
- Node-API add-ons run in Chord's embedded Bun runtime; check that the add-on targets Bun's supported Node-API surface.
- Accessibility calls inherit Chord's Accessibility permission—grant it to Chord.
- A native crash disables that package worker until configuration reload. Run the handler from the CLI while developing to inspect its diagnostics.

## FAQ

### Why Bun rather than a lighter runtime?

Bun brings JavaScriptCore's JIT and Node/Bun APIs, including the Node-API loader used by NodeSwift add-ons. Bun has no official embedding API, so Chord embeds a lightly patched Bun runtime through the [`rbun`](https://github.com/KeyChord/rbun) crate and exposes the custom `chord` module to scripts.

### Compact help ranges

A help hint can end in a parenthesized letter or digit range:

```toml
[chords]
'?-(a-z)' = { name = "Menu item by letter" }
```

After pressing `-`, the help panel shows one row with separate `A` and `Z`
keycaps and a dash between them. The row replaces individual options in that
range and highlights when any covered key is selected. Ranges are inclusive,
ascending, and use lowercase letters (`a-z`) or digits (`0-9`). The prefix
before the range is a normal key sequence. Hints describe options; they do not
create executable chord bindings.
