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
