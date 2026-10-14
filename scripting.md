# JavaScript

## Recommended Setup

TODO

## Recommended Packages

- [bplist-lossless](https://github.com/leonsilicon/bplist-lossless) - A binary plist parser specifically tailored for edits by avoiding loss of precision during parsing and re-serialization.
- [doctor-json](https://github.com/privatenumber/doctor-json) - A JSON editor that preserves all existing formatting/comments
- [keycode-ts2](https://github.com/leonsilicon/keycode-ts2) - A TypeScript port of the [Rust `keycode` crate](https://crates.io/crates/keycode) which uses the Chromium keycode names as the source of truth (_Chords_ uses these keycode names as the source of truth).

## Chord events and persistent return values

### Meta commands, repetition, and special keys

The leading `0` namespace belongs to Chord in every focused app. Its bindings and help
are defined in `@keychord/chords-chord/chords/com/leonsilicon/chord/macos.toml`.
This protected core file is always loaded, even before optional repositories load,
and cannot be removed, deselected, or overridden. Other files cannot define `0`
commands, hints, or `builtin` actions, and user regexes cannot intercept them.
Complete built-in commands with the normal chord completion controls or the CLI:

| Command | Action |
| --- | --- |
| `0s` | Open Chord Settings |
| `0r` | Reload configuration and stop active loops |
| `0q` | Quit Chord |
| `0h<ID>` | Halt the loop with that numeric ID, for example `0h12` |

These replace the former backslash Settings, Reload, and Quit sequences.

The core file maps chords to native app operations using `builtin` actions:

```toml
[chords]
'0s' = { name = "Settings", builtin = { command = "settings" } }
'0h(\d+)' = { name = "Halt loop #$1", builtin = { command = "halt-loop", argument = "$1" } }
```

The supported operations are `settings`, `reload`, `quit`, and `halt-loop`.
Only the protected core file has permission to use them. Installed builds bundle
the file so the controls remain available without external package sources.

Leading decimal digits set a chord's repeat count: `12ab` executes `ab` twelve times.
Without a prefix the chord executes once; the prefix `1` starts an indefinite loop.
For example, `1ab` keeps executing `ab` after the keys are released, chord mode ends,
or another chord runs. Each loop receives a distinct ID displayed in the chord panel
alongside its `0h<ID>` command. Halting a loop stops further iterations. An in-flight
JavaScript callback finishes before its values are released, preserving other handlers
in the same package worker. Shell commands are cancelled and simulated shortcut keys
are released. A failed iteration ends its loop; configuration reload and quitting stop
all loops. IDs are not reused during an app session.

Repeat counts apply to completed input actions; init and keyboard callbacks still run
once per matching interval or keyboard event. Regex capture arguments exclude the
repeat prefix: `12hsr34` runs `hsr34` twelve times with `$1` equal to `34`. Handlers
receive the underlying command as `this.input`, excluding the repeat prefix. Loop
iterations share their initialization values until halted. Iterations are sequential,
with a 10 ms pause between them so immediate actions leave time for other input.

Backslash introduces special-key notation: `\up`, `\left`, `\down`, and `\right`
represent arrow keys in TOML triggers and CLI sequences. For example, `'a\up'`
matches `a` followed by the Up arrow, or the typed escape sequence. Backslash is
reserved for special keys; bare or unknown escapes cannot define chords or help
hints. While typing an escape, the input stays pending until the special key is
complete. Inside parenthesized regex sections, backslash keeps its regex meaning
(for example, `(\d+)`). Use literal TOML strings (single quotes) to preserve
backslashes. Backspace continues to edit the chord buffer.

### Handler events

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

| Trigger           | When it calls the handler                                                                     |
| ----------------- | --------------------------------------------------------------------------------------------- |
| `@init:event`     | Once when this chord starts matching, before its other event handlers.                        |
| `@keydown:event`  | After each keydown, using the buffer after any edit.                                          |
| `@keypress:event` | After the matching keydown handlers, for every key press, including Backspace and modifiers.  |
| `@keyup:event`    | After each key release, using the current buffer.                                             |
| `@input:event`    | When the chord is completed with Caps Lock, the final shifted key, Space release, or the CLI. |

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
export function getScreenBounds(): WindowBounds;
export function mouse(options: {
  action: "click" | "down" | "move" | "up";
  windowId?: number;
  x?: number;
  y?: number;
  expectedBounds?: WindowBounds;
  button?: "left" | "right" | "middle";
  clickCount?: 1 | 2;
  pressDurationMs?: number;
}): Promise<void>;
export function click(options: {
  windowId: number;
  x: number;
  y: number;
  expectedBounds?: WindowBounds;
  moveCursor?: boolean;
  button?: "left" | "right" | "middle";
  clickCount?: 1 | 2;
  pressDurationMs?: number;
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

`click` sends button down/up events to that window. It defaults to one left click;
`button` selects left, right, or middle, and `clickCount: 2` sends a double click.
`pressDurationMs` sets the hold duration (default 80 ms, accepted range 1–5000 ms). `x` and `y` are window-local macOS
logical screen points measured from its top-left corner; they are independent of Retina scaling.
The helper preserves the foreground app and, by default, the Mac pointer. `moveCursor: true`
moves the pointer to the target point after validation while events still go directly to the
window; it never clicks through the cursor. Optional `expectedBounds` rejects a
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
move event or activates the target. An explicitly requested cursor move uses
`CGWarpMouseCursorPosition` before the window-targeted button events. It does not fall back to
cursor-based clicks if the SPI is missing.

`mouse` adds separate button-down, movement, and button-up operations. Supply
`windowId` for window-local input with the same targeting and geometry checks as
`click`; omit it for ordinary global macOS points. Global input reaches the visible
app under the pointer and can activate it. `getScreenBounds` returns the display
containing the pointer, using macOS logical points and including negative display
origins. Resolve all coordinates against a captured set of bounds before sending
input.

After `down`, `move` sends the corresponding left/right/middle dragged event.
`up` releases at the most recent successful point, even if the window has since
moved or disappeared. Mouse sessions belong to each package worker's host
connection; errors and worker disconnection release held buttons. A later normal
cursor-moving click releases a latched button before clicking. A `down` can stay
held across handler invocations; authors should otherwise release it in `finally`.
Standalone scripts share a session during their invocation and release it on exit.
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

`Sources/KeychordChordsMenuNativeMenu/menu.swift` → `target/<triple>/menu/menu.node`

`@keychord/config` builds the dynamic library products in the package's root `Package.swift` and resolves normal `.node` imports in handlers. Its plugin packages each addon under `js/native/<triple>/<name>/`, with NodeSwift's dynamic `libNodeAPI.dylib` beside it. Keeping that runtime dynamic lets Chord safely load add-ons from several packages in one process; the package does not need an npm dependency on NodeSwift at runtime.

> **Trust:** native code runs inside Chord with your full user permissions. Only enable packages from sources you trust.

## Package layout

```text
my-package/
├── chords/macos.toml
├── src/
│   ├── js/menu.ts                 # imports ./menu.node
│   └── types/native.d.ts          # declares the addon's TypeScript API
├── Package.swift                 # Swift products and dependencies
├── Sources/KeychordChordsMenuNativeMenu/
│   ├── menu.swift                 # entry point: exports #NodeModule
│   └── helpers.swift
├── js/
│   ├── menu.js                    # generated JavaScript bundle
│   └── native/aarch64-apple-darwin/menu/
│       ├── menu-<hash>.node       # packaged addon
│       └── libNodeAPI.dylib
└── target/                        # platform-specific output — commit/publish it like js/
    └── aarch64-apple-darwin/
        └── menu/
            ├── menu.node          # Node-API add-on
            └── libNodeAPI.dylib   # shared NodeSwift runtime, found through @loader_path
```

`src/` and `Sources/` are authored source, `chords/` is Chord configuration, and `js/` holds the generated bundles and their native assets. `target/<triple>/` holds native build products. Native build state lives in `node_modules/.cache/chord/native/`, covered by the usual `node_modules/` ignore. Keep distributable `js/` and `target/` files tracked.

## Writing one

Import `NodeAPI` and declare the add-on exports with `#NodeModule`. NodeSwift converts supported Swift values and thrown errors at the Node-API boundary, so no C ABI or manual pointer ownership is needed:

```swift
// Sources/KeychordExampleNativeBeep/beep.swift
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

The handler imports the addon normally. `config()` supplies the native-addon plugin automatically, so the handler needs no build alias or runtime path resolver:

```ts
// src/js/beep.ts
import addon from "./beep.node";

export default function build(times = 1) {
  return function beep() {
    addon.beep(times);
  };
}
```

Declare the imported API in `src/types/native.d.ts`:

```ts
declare module "*/beep.node" {
  const addon: { beep(times: number): void };
  export default addon;
}
```

- The plugin packages each configured target triple and selects the matching platform and architecture at runtime. Static imports load the addon with the handler module; a `require("./beep.node")` inside a function keeps loading lazy. Generated native asset URLs are relative to the emitted JavaScript, including after vendoring.
- Existing handlers may still use `import.meta.chord.resolveNative("beep")`, which resolves `target/<Chord's triple>/beep/beep.node`. `import.meta.chord.resolvePackageFile("any/relative/path")` resolves arbitrary package files. Both remain bound to the calling module's logical package.
- Each installed package runs in its own supervised process with a persistent embedded Bun VM. Imports, handler factories, lifecycle callbacks, timers, and native add-ons all stay in that process. On macOS the worker services AppKit events and Swift's `@MainActor`; keep NodeAPI values on `@NodeActor` and pass Swift values to the main actor. Return or await native Promises so Chord waits for completion and reports errors.
- JavaScript and Swift exceptions are reported as handler errors. A native abort, bad pointer, or `process.exit()` terminates only the package worker. Its supervisor automatically starts a fresh VM and restores the registered handler factories in their original order. Existing handler IDs remain usable, other packages keep running, and the interrupted action is never replayed. Repeated crashes or startup failures use exponential backoff from 250 ms to 30 seconds, resetting after a healthy minute. Setup and health checks have a 15-second deadline; actions and lifecycle callbacks have a five-minute deadline for interactive UI.
- Recovery resets the package's JavaScript globals, sequence return values, timers, and native state. Factories run again to rebuild handlers and lifecycle subscriptions; keep factories suitable for reinitialization. Unloading/reloading configuration and quitting stop the supervisor and prevent it from respawning workers.
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

Vendor the dependency with `config({ vendor: ["@keychord/chords-menu"] })` and import its JavaScript normally. Its copied `js/` tree includes the packaged native addons and runtime libraries, so relative native asset URLs continue to work. Vendoring also preserves the `target/` tree for existing handlers using runtime path resolvers. Add-ons are consumed through their JavaScript API, not by importing their generated Swift modules.

For compatibility with installed packages, the `chord` module still exports `resolveNativeModulePath(import.meta, relpath)` and `resolvePackageFile(import.meta, path)`. These delegate to the same per-module resolvers. New native handlers built with `@keychord/config` can use `.node` imports directly.

## Building

Packages built with `@keychord/config` compile the package's `Package.swift` automatically as part of `vp pack` (and rebuild on change in `vp pack --watch`). To build every distributable folder at once—`js/` and `target/<triple>/`—run `vpr compile` (`--triple <triple>` to cross-build, `--skip-js`/`--skip-native` for one half, `vpr -r compile` for the whole workspace). Building needs a Swift toolchain from Xcode or the Command Line Tools. Build options go in `vite.config.ts`; Swift frameworks, compiler flags, and linker settings belong in `Package.swift`:

```ts
import { config } from "@keychord/config";

export default config({
  native: {
    triples: ["aarch64-apple-darwin", "x86_64-apple-darwin"], // default: host triple
  },
});
```

`@keychord/config` builds the package's root `Package.swift` directly. SwiftPM build products and staging files live in each package's `node_modules/.cache/chord/native/`, grouped by target triple and add-on; distributable files are copied to `target/`. `@keychord/config` currently supports Apple triples.

Any other build system works too: Chord imports the package's JavaScript from disk, and its loader must resolve a Node-API addon compatible with Bun plus any dynamic dependencies. For the standard NodeSwift build, `libNodeAPI.dylib` sits beside the addon and is found through `@loader_path`.

## Testing without the app

A Chord build's CLI runs a script on the same embedded Bun, with the `chord` module available. Test scripts should import the generated handler from `js/` so its native imports have been processed by the plugin:

```sh
chord bun scripts/run.ts by-letters f     # from the chords-menu checkout
```

## Troubleshooting

- Native loading failures name the missing file or Node-API registration problem; run `vp pack` and check that the packaged addon and `libNodeAPI.dylib` under `js/native/<your triple>/` are included and that the `#NodeModule` export names match the TypeScript binding.
- Node-API add-ons run in Chord's embedded Bun runtime; check that the add-on targets Bun's supported Node-API surface.
- Accessibility calls inherit Chord's Accessibility permission—grant it to Chord.
- A native crash reports failure for the interrupted action; the worker restarts automatically for later actions. Run the handler from the CLI while developing to inspect its diagnostics.

## FAQ

### Why Bun rather than a lighter runtime?

Bun brings JavaScriptCore's JIT and Node/Bun APIs, including the Node-API loader used by NodeSwift add-ons. Bun has no official embedding API, so Chord embeds a lightly patched Bun runtime through the [`rbun`](https://github.com/KeyChord/rbun) crate and exposes the custom `chord` module to scripts.

### Compact help ranges

A help hint can end in a quoted letter or digit range:

```toml
[chords]
'?"1-9"' = { name = "Number" }
'?m"0-9"' = { name = "Menu item number" }
'?-"a-z"' = { name = "Menu item by letter" }
```

The first hint shows one physical keycap containing `1-9`. After pressing `m`,
the second shows `0-9`; after `-`, the third shows `A-Z`. Each row replaces
individual options in that range and highlights when any covered key is selected. Ranges are inclusive,
ascending, and use lowercase letters (`a-z`) or digits (`0-9`). The prefix
before the range is a normal key sequence. Hints describe options; they do not
create executable chord bindings. Parenthesized hints such as `?-(a-z)` remain
supported and use the same compact keycap display.

The core `@keychord/chords-chord` package defines repeat-count help with
`'?"1-9"' = { name = "Repeat count" }`. The panel extends this range to `0-9`
after a numeric prefix. At the start of a sequence, `0` (Chord settings) and
`1` (Loop chord) also keep their dedicated rows.
