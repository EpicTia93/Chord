# Project Environment

- Chord reserves `0` for built-in global meta commands: `0s` Settings, `0r` reload, `0q` quit, and `0h<ID>` halt loop. Leading numeric prefixes repeat input actions; `1` starts an independent loop. Loop IDs and help live in the `chord-execution` observable owned by the chord-mode manager; normal input resets preserve loops, while reload/exit stop them. In-flight JavaScript callbacks finish on halt to preserve other handlers in their package worker. `\up`, `\left`, `\down`, and `\right` parse as physical arrow keys. The former backslash meta definitions were removed from `../chords/@keychord/chords-chord`.

- Chord is a Bun workspace monorepo with a Tauri 2 desktop app in `apps/chord-tauri`.
- The frontend is React 19, TypeScript, Vite, and Tailwind CSS 4; it is not React Native.
- Run the app from the repository root with `bun run dev`; Vite uses port 1420.
- Run the frontend build with `bun run build` and Rust tests from `apps/chord-tauri/src-tauri` with `cargo test`.
- The settings window is created in `apps/chord-tauri/src-tauri/src/app/settings/settings_ui.rs` with label `settings`.
- The settings layout lives in `packages/dev/improve/chord/routes/settings/settings-page.tsx`.
- Route adapters stay in `apps/chord-tauri/src/routes`; reusable UI belongs in workspace packages.
- The existing sidebar primitive is `@chord/dev.improve.chord.components.ui.sidebar`.
- Native macOS translucency is available through Tauri window effects; `window-vibrancy` 0.8 is also already present.

<!-- argent-environment-inspector: 2026-10-12 -->
```json
{"project_type":"Tauri 2 native macOS desktop app with React web frontend","is_react_native":false,"is_native_ios":false,"is_native_android":false,"mobile_platforms":[],"package_manager":"bun@1.4.2","toolchain":{"bun":"1.4.2","rust":"1.98.0","cargo":"1.98.0","node":"24.21.0","proto":"0.61.1"},"commands":{"start":"bun run dev","frontend_build":"bun run build","rust_check":"cargo check --manifest-path apps/chord-tauri/src-tauri/Cargo.toml","rust_tests":"cargo test --manifest-path apps/chord-tauri/src-tauri/Cargo.toml --lib","runtime_launcher_tests":"bun test ./packages/dev/improve/chord/configs/dev/tauri/runtime.test.ts"},"frontend_port":1420,"metro_port":null,"runtime":{"integration":"apps/chord-tauri/src-tauri/src/bun_js/","rbun_dependency":"../rbun/crates/rbun","development_bundle_id":"com.leonsilicon.chord.development","start_behavior":"Signed immutable app snapshot launched through Launch Services; replaces running development instance."},"argent":"Available; device workflows do not apply to native macOS AppKit/Tauri."}
```

- 2026-10-12 crash investigation: reproduced NodeSwift 1.5.1's `NodeExecutor.enqueue` abort after an async `NodeActor` → `MainActor` hop with multiple addon environments. The tray and menu packages in `../chords/@keychord/` now return promises from synchronous NodeFunction entrypoints and capture an explicit NodeAsyncQueue for completion. Do not reintroduce async NodeFunction wrappers around MainActor work. Regression: `CHORD_NATIVE_TEST_HOST=<Chord executable> bun test __tests__/actor-queue.test.ts` from the tray package.
- Caps+Space activation uses `CapturedKeyEvent`'s Caps state at capture time; queued Caps events must not overwrite the newer physical HID state. Native artifact staging uses rename so rebuilds preserve images mapped by a running process.
- 2026-10-13: Fixed the same NodeSwift missing-queue abort in iPhone Mirroring using synchronous NodeFunction entrypoints, captured NodeAsyncQueue handles, and explicit promise completion. The Apple read-only actor-queue regression validates 500 value/void/error completions with multiple addons. Package workers now have stable supervisors that restore handler factories in registration order after crashes/timeouts, retain logical handler IDs, and never replay interrupted actions. Retry backoff is 250 ms–30 s and resets after a healthy minute; stop/unload prevents respawn. Tests: cargo test --lib (119 pass), supervisor_ --ignored (2 pass), package_faults_are_contained --ignored (1 pass).
