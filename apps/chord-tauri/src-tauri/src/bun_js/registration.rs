//! Handler factories execute exclusively in the package child process.
use rbun::prelude::{Args, Object, OptionExt, Value};
use rbun::{Ctx, Module};

pub async fn register<'js>(
    ctx: Ctx<'js>,
    module_specifier: String,
    raw: serde_json::Value,
    pathslug_string: String,
    bundle_id: Option<String>,
    build_args: Vec<toml::Value>,
) -> rbun::Result<String> {
    let module_promise = Module::import(&ctx, module_specifier)?;
    let module = module_promise.into_future::<Object>().await?;
    let mut export: Value = module.get("default")?;

    if let Some(promise) = export.as_promise().cloned() {
        export = promise.into_future::<Value>().await?;
    }

    let build_handler_function = export.as_function().cloned().or_throw_msg(
        &ctx,
        &format!(
            "JS default export did not resolve to a function: {:?}",
            export
        ),
    )?;

    let build_context = Object::new(ctx.clone())?;
    build_context.set(
        "chordsFile",
        rbun::serde::to_value(ctx.clone(), raw)
            .or_throw_msg(&ctx, "failed to parse chords file")?,
    )?;
    build_context.set(
        "chordsFilePath",
        rbun::serde::to_value(ctx.clone(), pathslug_string)
            .or_throw_msg(&ctx, "failed to parse chords file pathslug")?,
    )?;
    build_context.set(
        "chordsFileAppId",
        rbun::serde::to_value(ctx.clone(), bundle_id)
            .or_throw_msg(&ctx, "failed to parse chords file app ID")?,
    )?;

    let mut args = Args::new(ctx.clone(), build_args.len());
    args.this(build_context)?;
    log::debug!("calling build_handler with args {:?} (bun)", build_args);

    let js_args = build_args
        .into_iter()
        .map(|value| {
            rbun::serde::to_value(ctx.clone(), value)
                .or_throw_msg(&ctx, "failed to convert event TOML arguments")
        })
        .collect::<rbun::Result<Vec<_>>>()?;

    for value in js_args {
        args.push_arg(value)?;
    }

    let mut handler: Value = build_handler_function.call_arg(args)?;
    if let Some(promise) = handler.as_promise().cloned() {
        handler = promise.into_future::<Value>().await?;
    }

    let handler_function = handler
        .as_function()
        .cloned()
        .or_throw_msg(&ctx, "the default export function must return a function")?;
    let globals = ctx.globals();
    let registry_key = "__RUST_HANDLER_REGISTRY";

    // Fetch the global registry object, or create it if it doesn't exist
    let registry: Object = match globals.get(registry_key) {
        Ok(obj) => obj,
        Err(_) => {
            let obj = Object::new(ctx.clone())?;
            obj.set_prototype(None)?;
            globals.set(registry_key, obj.clone())?;
            obj
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    registry.set(&id, handler_function)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::chord_runner::invoke_js_handler;
    use crate::bun_js::{format_js_error, with_test_js};
    use crate::models::HandlerChordAction;

    #[tokio::test]
    async fn repetitions_build_the_handler_once_and_share_the_run_window() {
        let root = std::env::temp_dir().join(format!("chord-repeat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let module = root.join("window.js");
        std::fs::write(
            &module,
            r#"
export default function buildWindowHandler() {
    globalThis.repeatBuilds = (globalThis.repeatBuilds ?? 0) + 1;
    return async function window(direction) {
        await Promise.resolve();
        (globalThis.repeatCalls ??= []).push(`${direction}@${this.focusedWindowId}`);
    };
}
"#,
        )
        .unwrap();
        let specifier = module.to_string_lossy().into_owned();
        let result = with_test_js(|ctx| {
            Box::pin(async move {
                let result = async {
                    let id = register(
                        ctx.clone(),
                        specifier,
                        serde_json::json!({ "name": "fixture" }),
                        "chords/macos.toml".into(),
                        None,
                        vec![],
                    )
                    .await?;
                    let mut space = HandlerChordAction {
                        handler_id: id,
                        event_args: vec!["NEXT".into()],
                        input: "/ws]".into(),
                        sequence_id: None,
                        assign_to: None,
                        keyboard_event: None,
                        focused_window_id: Some(42),
                    };
                    // `3/ws]` repeats inside one task.
                    invoke_js_handler(ctx.clone(), &space, Some("dev.fixture".into()), 3).await?;
                    // Loops and paused runs start each iteration as a task of their own.
                    space.focused_window_id = Some(7);
                    for _ in 0..3 {
                        invoke_js_handler(ctx.clone(), &space, Some("dev.fixture".into()), 1)
                            .await?;
                    }
                    // Without a captured window, handlers see null rather than a stale value.
                    space.focused_window_id = None;
                    invoke_js_handler(ctx.clone(), &space, None, 1).await?;
                    assert_eq!(ctx.eval::<u32, _>("globalThis.repeatBuilds")?, 1);
                    assert_eq!(
                        ctx.eval::<String, _>("globalThis.repeatCalls.join(',')")?,
                        "NEXT@42,NEXT@42,NEXT@42,NEXT@7,NEXT@7,NEXT@7,NEXT@null"
                    );
                    Ok::<(), rbun::Error>(())
                }
                .await;
                result.map_err(|error| anyhow::anyhow!(format_js_error(&ctx, error)))
            })
        })
        .await;
        let _ = std::fs::remove_dir_all(&root);
        result.unwrap();
    }
}
