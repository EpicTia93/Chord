//! The `chord` built-in module exposed by the embedded Bun runtime.

use crate::app::AppHandleExt;
use crate::app::chord_package_manager::resolve_logical_package_path;
use crate::app::global_hotkey_store::GlobalHotkeyStoreEntry;
use crate::bun_js::AppUserData;
use crate::bun_js::lifecycle::{
    init_app_lifecycle, register_app_launch_handler, register_app_terminate_handler,
};
use crate::constants::GLOBAL_HOTKEYS_POOL;
use crate::models::{ShortcutChordAction, SimulatedShortcut, native_module_relpath};
#[cfg(target_os = "macos")]
use osakit::{Language as OsaLanguage, Script as OsaScript};
use rbun::prelude::*;
use rbun::{Array, Class, Ctx, Exception, Function, JsLifetime, Object, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct ChordModule;

enum ApplescriptLanguage {
    AppleScript,
    JavaScript,
}

#[rbun::class]
#[derive(rbun::derive::Trace)]
struct Applescript {
    #[cfg(target_os = "macos")]
    script: OsaScript,
    #[cfg(not(target_os = "macos"))]
    source: String,
}

unsafe impl<'js> JsLifetime<'js> for Applescript {
    type Changed<'to> = Applescript;
}

#[rbun::methods(rename_all = "camelCase")]
impl Applescript {
    #[qjs(constructor)]
    fn new<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rbun::Result<Self> {
        let mut args_iter = args.0.into_iter();
        let Some(first_arg) = args_iter.next() else {
            return Err(Exception::throw_message(
                &ctx,
                "Applescript constructor expects a source string or function",
            ));
        };

        if let Some(source) = first_arg.as_string() {
            if args_iter.next().is_some() {
                return Err(Exception::throw_message(
                    &ctx,
                    "Applescript only accepts extra arguments when constructed from a function",
                ));
            }

            return Ok(Self::from_source(
                ApplescriptLanguage::AppleScript,
                source.to_string()?,
            ));
        }

        if let Some(function) = first_arg.as_function() {
            let function_source = function_source(&ctx, function)?;
            let wrapped_source = wrap_jxa_function(&ctx, function_source, args_iter.collect())?;
            return Ok(Self::from_source(
                ApplescriptLanguage::JavaScript,
                wrapped_source,
            ));
        }

        Err(Exception::throw_message(
            &ctx,
            "Applescript constructor expects a source string or function",
        ))
    }

    fn compile<'js>(&mut self, ctx: Ctx<'js>) -> rbun::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.script.compile().map_err(|error| {
                Exception::throw_message(&ctx, &format!("failed to compile AppleScript: {error}"))
            })?;
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = &self.source;
            Err(Exception::throw_message(
                &ctx,
                "AppleScript is only supported on macOS",
            ))
        }
    }

    fn execute<'js>(&self, ctx: Ctx<'js>) -> rbun::Result<Value<'js>> {
        #[cfg(target_os = "macos")]
        {
            let value = self.script.execute().map_err(|error| {
                Exception::throw_message(&ctx, &format!("failed to execute AppleScript: {error}"))
            })?;

            rbun::serde::to_value(ctx.clone(), value)
                .or_throw_msg(&ctx, "failed to convert AppleScript result")
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = &self.source;
            Err(Exception::throw_message(
                &ctx,
                "AppleScript is only supported on macOS",
            ))
        }
    }
}

impl Applescript {
    fn from_source(language: ApplescriptLanguage, source: String) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            script: OsaScript::new_from_source(
                match language {
                    ApplescriptLanguage::AppleScript => OsaLanguage::AppleScript,
                    ApplescriptLanguage::JavaScript => OsaLanguage::JavaScript,
                },
                &source,
            ),
            #[cfg(not(target_os = "macos"))]
            source,
        }
    }
}

fn function_source<'js>(ctx: &Ctx<'js>, function: &Function<'js>) -> rbun::Result<String> {
    let to_string: Function<'js> = function
        .get("toString")
        .or_throw_msg(ctx, "failed to read function.toString")?;

    to_string
        .call((This(function.clone()),))
        .or_throw_msg(ctx, "failed to stringify function source")
}

fn wrap_jxa_function<'js>(
    ctx: &Ctx<'js>,
    function_source: String,
    args: Vec<Value<'js>>,
) -> rbun::Result<String> {
    let serialized_args = serialize_jxa_args(ctx, args)?;

    Ok(format!(
        "var __chordArgs = {serialized_args};\nvar __chordFn = ({function_source});\noutput = __chordFn.apply(null, __chordArgs);"
    ))
}

fn serialize_jxa_args<'js>(ctx: &Ctx<'js>, args: Vec<Value<'js>>) -> rbun::Result<String> {
    let array = Array::new(ctx.clone())?;
    for (index, value) in args.into_iter().enumerate() {
        array.set(index, value)?;
    }

    ctx.json_stringify(array.into_value())?
        .map(|json| json.to_string())
        .transpose()?
        .ok_or_else(|| Exception::throw_message(ctx, "failed to serialize JXA arguments"))
}

fn app_handle(ctx: &Ctx<'_>) -> rbun::Result<tauri::AppHandle> {
    let userdata = ctx
        .userdata::<AppUserData>()
        .ok_or_else(|| Exception::throw_message(ctx, "missing app context"))?;

    userdata
        .handle
        .clone()
        .ok_or_else(|| Exception::throw_message(ctx, "`chord` module is unavailable in CLI mode"))
}

fn on_app_launch<'js>(
    ctx: Ctx<'js>,
    bundle_id: String,
    callback: Function<'js>,
) -> rbun::Result<()> {
    let _ = app_handle(&ctx)?;
    register_app_launch_handler(ctx, bundle_id, callback)
}

fn on_app_terminate<'js>(
    ctx: Ctx<'js>,
    bundle_id: String,
    callback: Function<'js>,
) -> rbun::Result<Function<'js>> {
    let _ = app_handle(&ctx)?;
    register_app_terminate_handler(ctx, bundle_id, callback)
}

fn press(ctx: Ctx, key: String) -> rbun::Result<()> {
    let handle = app_handle(&ctx)?;
    let runner = handle.app_state().chord_action_task_runner();
    let simulated_shortcut = key
        .parse::<SimulatedShortcut>()
        .or_throw_msg(&ctx, &format!("Invalid shortcut {}", key))?;
    runner
        .shortcut
        .simulate_shortcut_actions(
            runner.shortcut.get_start_simulated_shortcut_actions(
                &ShortcutChordAction { simulated_shortcut },
                1,
            ),
        )
        .or_throw_msg(&ctx, "failed to press shortcut")?;
    Ok(())
}

fn release(ctx: Ctx, key: String) -> rbun::Result<()> {
    let handle = app_handle(&ctx)?;
    let runner = handle.app_state().chord_action_task_runner();
    let simulated_shortcut = key
        .parse::<SimulatedShortcut>()
        .or_throw_msg(&ctx, &format!("Invalid shortcut {}", key))?;
    runner
        .shortcut
        .simulate_shortcut_actions(
            runner
                .shortcut
                .get_end_simulated_shortcut_actions(&ShortcutChordAction { simulated_shortcut }),
        )
        .or_throw_msg(&ctx, "failed to release shortcut")?;
    Ok(())
}

fn tap(ctx: Ctx, key: String) -> rbun::Result<()> {
    press(ctx.clone(), key.clone())?;
    release(ctx.clone(), key)?;
    Ok(())
}

fn get_global_hotkey(
    ctx: Ctx,
    bundle_id: String,
    hotkey_id: String,
) -> rbun::Result<Option<String>> {
    let handle = app_handle(&ctx)?;
    let global_hotkey_store = handle.app_state().global_hotkey_store();
    let shortcut = global_hotkey_store
        .entries()
        .or_throw_msg(&ctx, "bad entries")?
        .into_iter()
        .find_map(|(shortcut, entry)| {
            (entry.bundle_id == bundle_id && entry.hotkey_id == hotkey_id).then_some(shortcut)
        });

    Ok(shortcut)
}

fn register_global_hotkey(
    ctx: Ctx,
    bundle_id: String,
    hotkey_id: String,
) -> rbun::Result<Option<String>> {
    let handle = app_handle(&ctx)?;
    let global_hotkey_store = handle.app_state().global_hotkey_store();
    let all = global_hotkey_store
        .entries()
        .or_throw_msg(&ctx, "bad store")?;

    // idempotent: if this hotkey is already registered, return the existing shortcut
    if let Some(existing) = all.iter().find_map(|(shortcut, entry)| {
        (entry.bundle_id == bundle_id && entry.hotkey_id == hotkey_id).then_some(shortcut.clone())
    }) {
        return Ok(Some(existing));
    }

    let used: HashSet<String> = all.into_keys().collect();

    let Some(next) = GLOBAL_HOTKEYS_POOL
        .iter()
        .find(|shortcut| !used.contains(&shortcut.serialize()))
        .cloned()
    else {
        return Ok(None);
    };

    let shortcut = next.serialize();

    global_hotkey_store
        .set(
            &shortcut,
            GlobalHotkeyStoreEntry {
                bundle_id,
                hotkey_id,
            },
        )
        .or_throw_msg(&ctx, "failed to save global hotkey")?;

    Ok(Some(shortcut))
}

fn set_app_needs_relaunch(ctx: Ctx, bundle_id: String, needs_relaunch: bool) -> rbun::Result<()> {
    let handle = app_handle(&ctx)?;
    let desktop_app_manager = handle.app_state().desktop_app_manager();
    desktop_app_manager
        .set_app_needs_relaunch(&bundle_id, needs_relaunch)
        .or_throw_msg(&ctx, "failed to set app relaunch flag")?;
    Ok(())
}

fn get_app_windows<'js>(ctx: Ctx<'js>, bundle_id: String) -> rbun::Result<Value<'js>> {
    let windows = crate::native_input::get_app_windows(&bundle_id)
        .or_throw_msg(&ctx, "failed to enumerate application windows")?;
    rbun::serde::to_value(ctx.clone(), windows)
        .or_throw_msg(&ctx, "failed to convert application windows")
}

async fn click<'js>(ctx: Ctx<'js>, options: Value<'js>) -> rbun::Result<()> {
    let options: crate::native_input::ClickOptions =
        rbun::serde::from_value(options).or_throw_msg(&ctx, "invalid click options")?;
    tauri::async_runtime::spawn_blocking(move || crate::native_input::click(&options))
        .await
        .or_throw_msg(&ctx, "click worker failed")?
        .or_throw_msg(&ctx, "window-targeted click failed")
}

async fn run_sudo_command<'js>(
    ctx: Ctx<'js>,
    program: String,
    args: Array<'js>,
) -> rbun::Result<()> {
    let mut rust_args: Vec<String> = Vec::new();
    for item in args.into_iter() {
        let arg_string: String = item?.get()?;
        rust_args.push(arg_string);
    }

    let thread_result = tauri::async_runtime::spawn_blocking(move || {
        if elevated_command::Command::is_elevated() {
            std::process::Command::new(program)
                .args(rust_args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|error| format!("elevated command failed: {error}"))
        } else {
            // elevated-command captures its child's complete output. Run Chord's small helper
            // under elevation instead; the helper discards the target command's output so the
            // elevation bridge can never accumulate it in memory or in its temporary files.
            let chord_executable = std::env::current_exe()
                .map_err(|error| format!("failed to locate Chord executable: {error}"))?;
            let mut helper = std::process::Command::new(chord_executable);
            helper.arg("__elevated-exec");
            helper.arg(encode_elevated_argument(&program));
            helper.args(rust_args.iter().map(|arg| encode_elevated_argument(arg)));

            let mut elevated_cmd = elevated_command::Command::new(helper);
            elevated_cmd.name("Chord".to_string());
            elevated_cmd
                .output()
                .map(|output| output.status)
                .map_err(|error| format!("elevated command failed: {error}"))
        }
    })
    .await;

    match thread_result {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(Exception::throw_message(
            &ctx,
            &format!("elevated command exited with status {status}"),
        )),
        Ok(Err(e)) => Err(Exception::throw_message(
            &ctx,
            &format!("failed to run command: {}", e),
        )),
        Err(e) => Err(Exception::throw_message(
            &ctx,
            &format!("background thread failed: {}", e),
        )),
    }
}

fn encode_elevated_argument(argument: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(1 + argument.len() * 2);
    encoded.push('x');
    for byte in argument.as_bytes() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

/// `(package root, module path relative to it)` for the module at `module_path`.
///
/// Inside the app the package is found through the package manager (the module was imported
/// from the package directory, so its path is under a known root). Standalone
/// (`chord bun file.ts`) the nearest ancestor with a `package.json` is taken as the root, so
/// packages can be exercised from a checkout.
fn locate_module_path<'js>(ctx: &Ctx<'js>, module_path: &Path) -> rbun::Result<(PathBuf, PathBuf)> {
    let module_path = module_path.to_path_buf();

    if let Ok(handle) = app_handle(ctx) {
        let package_manager = handle.app_state().chord_package_manager();
        if let Some((_, root)) = package_manager.package_for_path(&module_path) {
            let relative = module_path
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .or_throw_msg(ctx, "module is not inside its package root")?;
            return Ok((root, relative));
        }
    }

    for ancestor in module_path.ancestors().skip(1) {
        if ancestor.join("package.json").is_file() {
            let relative = module_path
                .strip_prefix(ancestor)
                .map(Path::to_path_buf)
                .or_throw_msg(ctx, "module is not inside its package root")?;
            return Ok((ancestor.to_path_buf(), relative));
        }
    }

    Err(Exception::throw_message(
        ctx,
        &format!("{} is not inside a chord package", module_path.display()),
    ))
}

/// The absolute path of a file of `module_path`'s package, given relative to the package root
/// as authored (vendored packages are remapped to where their folders were copied). The file
/// need not exist.
fn package_file_for<'js>(
    ctx: &Ctx<'js>,
    module_path: &Path,
    relative: &str,
) -> rbun::Result<String> {
    let (root, module_relpath) = locate_module_path(ctx, module_path)?;
    let resolved = resolve_logical_package_path(&module_relpath, Path::new(relative))
        .or_throw_msg(ctx, "invalid package-relative path")?;
    Ok(root.join(resolved).to_string_lossy().into_owned())
}

/// Build the `import.meta.chord` namespace for the module at `module_path`.
///
/// Every function closes over that path, which comes from Bun's own per-module
/// `import.meta` hook rather than from an argument. A module therefore cannot resolve as if it
/// were a different one, which the previous `chord` module exports (taking `import.meta` as a
/// plain value) could not prevent.
fn build_import_meta_namespace<'js>(
    ctx: &Ctx<'js>,
    module_path: PathBuf,
) -> rbun::Result<Object<'js>> {
    let namespace = Object::new(ctx.clone())?;

    // `import.meta.chord.resolvePackageFile("target/x/y")`
    let path = module_path.clone();
    namespace.set(
        "resolvePackageFile",
        Func::from(move |ctx: Ctx<'js>, relative: String| package_file_for(&ctx, &path, &relative)),
    )?;

    // `import.meta.chord.resolveNative("menu")`: the package's prebuilt Node-API add-on
    // `target/<this triple>/menu/menu.node`, ready for `process.dlopen`.
    let path = module_path;
    namespace.set(
        "resolveNative",
        Func::from(move |ctx: Ctx<'js>, relpath: String| {
            let module_relpath = native_module_relpath(&relpath)
                .or_throw_msg(&ctx, "invalid native module relative path")?;
            package_file_for(&ctx, &path, &module_relpath.to_string_lossy())
        }),
    )?;

    Ok(namespace)
}

/// Compatibility for installed handlers built before `import.meta.chord` was introduced.
/// Delegate to the module-bound helpers without trusting import.meta's mutable path or URL.
fn resolve_with_import_meta<'js>(
    ctx: &Ctx<'js>,
    meta: Object<'js>,
    name: &str,
    relative: String,
) -> rbun::Result<String> {
    let namespace: Object<'js> = meta.get("chord").or_throw_msg(
        ctx,
        "expected import.meta with Chord's per-module resolvers",
    )?;
    let resolve: Function<'js> = namespace.get(name).or_throw_msg(
        ctx,
        "expected import.meta with Chord's per-module resolvers",
    )?;
    resolve.call((This(namespace), relative))
}

/// Attach `import.meta.chord` to every module Bun loads from disk.
///
/// Install once per runtime, before any handler is imported. JavaScriptCore caches the
/// `import.meta` object this decorates, so the namespace persists for the module's lifetime.
pub(crate) async fn install_import_meta_hook(rt: &rbun::AsyncRuntime) {
    rt.set_import_meta_hook(Some(Box::new(|ctx, specifier, meta| {
        // The specifier is a filesystem path for modules loaded from disk, and a bare name
        // (`chord`) or synthetic id for the rest, which own no package and get no namespace.
        let Some(specifier) = specifier.as_string().and_then(|s| s.to_string().ok()) else {
            return;
        };
        let module_path = Path::new(specifier.strip_prefix("file://").unwrap_or(&specifier));
        if !module_path.is_absolute() {
            return;
        }
        if let Ok(namespace) = build_import_meta_namespace(&ctx, module_path.to_path_buf()) {
            let _ = meta.set("chord", namespace);
        }
    })))
    .await;
}

impl ModuleDef for ChordModule {
    fn declare(declare: &Declarations) -> rbun::Result<()> {
        declare.declare("Applescript")?;
        declare.declare("press")?;
        declare.declare("release")?;
        declare.declare("tap")?;
        declare.declare("click")?;
        declare.declare("getAppWindows")?;
        declare.declare("getGlobalHotkey")?;
        declare.declare("registerGlobalHotkey")?;
        declare.declare("setAppNeedsRelaunch")?;
        declare.declare("onAppLaunch")?;
        declare.declare("onAppTerminate")?;
        declare.declare("runSudoCommand")?;
        declare.declare("resolveNativeModulePath")?;
        declare.declare("resolvePackageFile")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> rbun::Result<()> {
        if let Ok(handle) = app_handle(ctx) {
            init_app_lifecycle(handle);
        }

        Class::<Applescript>::define(&ctx.globals())?;
        let applescript_ctor: Value<'js> = ctx.globals().get(Applescript::NAME)?;

        exports.export("Applescript", applescript_ctor)?;
        exports.export("press", Func::from(press))?;
        exports.export("release", Func::from(release))?;
        exports.export("tap", Func::from(tap))?;
        exports.export("click", Func::from(Async(click)))?;
        exports.export("getAppWindows", Func::from(get_app_windows))?;
        exports.export("getGlobalHotkey", Func::from(get_global_hotkey))?;
        exports.export("registerGlobalHotkey", Func::from(register_global_hotkey))?;
        exports.export("setAppNeedsRelaunch", Func::from(set_app_needs_relaunch))?;
        exports.export("onAppLaunch", Func::from(on_app_launch))?;
        exports.export("onAppTerminate", Func::from(on_app_terminate))?;
        exports.export("runSudoCommand", Func::from(Async(run_sudo_command)))?;
        exports.export(
            "resolveNativeModulePath",
            Func::from(|ctx: Ctx<'js>, meta: Object<'js>, relative: String| {
                resolve_with_import_meta(&ctx, meta, "resolveNative", relative)
            }),
        )?;
        exports.export(
            "resolvePackageFile",
            Func::from(|ctx: Ctx<'js>, meta: Object<'js>, relative: String| {
                resolve_with_import_meta(&ctx, meta, "resolvePackageFile", relative)
            }),
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bun_js::{format_js_error, with_test_js};
    use rbun::Module;

    #[tokio::test]
    async fn legacy_package_resolvers_use_the_bound_module_namespace() {
        with_test_js(|ctx| {
            Box::pin(async move {
                let result = async {
                    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
                    for (index, module_relpath) in
                        ["js/tray.js", "js/@keychord/chords-tray/js/tray.js"]
                            .into_iter()
                            .enumerate()
                    {
                        let meta = Object::new(ctx.clone())?;
                        meta.set(
                            "chord",
                            build_import_meta_namespace(&ctx, root.join(module_relpath))?,
                        )?;
                        // The old API accepts import.meta, but must never trust its mutable path.
                        meta.set("path", "/different-package/js/tray.js")?;
                        meta.set("url", "file:///different-package/js/tray.js")?;
                        ctx.globals().set("__legacyResolverMeta", meta)?;
                        Module::evaluate(
                            ctx.clone(),
                            format!("legacy_package_resolvers_{index}.js"),
                            r#"
import { resolveNativeModulePath, resolvePackageFile } from "chord";
globalThis.__legacyResolverPaths = {
    native: resolveNativeModulePath(globalThis.__legacyResolverMeta, "tray"),
    file: resolvePackageFile(globalThis.__legacyResolverMeta, "bin/helper")
};
for (const resolve of [resolveNativeModulePath, resolvePackageFile]) {
    for (const meta of [{ path: "/different-package/js/tray.js" }, { chord: {} }]) {
        let rejected = false;
        try { resolve(meta, "tray"); } catch { rejected = true; }
        if (!rejected) throw new Error("accepted import.meta without a bound resolver");
    }
}
for (const [resolve, path] of [
    [resolveNativeModulePath, "../tray"],
    [resolvePackageFile, "../outside"],
    [resolvePackageFile, "/outside"]
]) {
    let rejected = false;
    try { resolve(globalThis.__legacyResolverMeta, path); } catch { rejected = true; }
    if (!rejected) throw new Error("accepted a path outside the package");
}
"#,
                        )?
                        .into_future::<()>()
                        .await?;
                        let paths: Object = ctx.globals().get("__legacyResolverPaths")?;
                        let native: String = paths.get("native")?;
                        let file: String = paths.get("file")?;
                        let native_relpath = native_module_relpath("tray").unwrap();
                        assert_eq!(
                            PathBuf::from(native),
                            root.join(
                                resolve_logical_package_path(
                                    Path::new(module_relpath),
                                    &native_relpath,
                                )
                                .unwrap(),
                            )
                        );
                        assert_eq!(
                            PathBuf::from(file),
                            root.join(
                                resolve_logical_package_path(
                                    Path::new(module_relpath),
                                    Path::new("bin/helper"),
                                )
                                .unwrap(),
                            )
                        );
                    }
                    ctx.globals().remove("__legacyResolverMeta")?;
                    ctx.globals().remove("__legacyResolverPaths")?;
                    Ok::<(), rbun::Error>(())
                }
                .await;
                result.map_err(|error| anyhow::anyhow!(format_js_error(&ctx, error)))
            })
        })
        .await
        .unwrap();
    }
}
