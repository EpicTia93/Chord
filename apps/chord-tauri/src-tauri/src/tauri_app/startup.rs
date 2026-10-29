pub(crate) const APP_STATE_STORE_PATH: &str = "app-state.json";

pub fn is_autostart_launch() -> bool {
    is_autostart_args(std::env::args_os())
}

pub fn is_autostart_args<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == "--autostart")
}

/// macOS permission checks are async on Tauri's runtime; `tauri::async_runtime::block_on` must not
/// run on a Tokio worker (e.g. from an IPC handler), so we isolate checks on a short-lived runtime.
fn required_permissions_granted_sync() -> bool {
    match std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build permission-check runtime");
        rt.block_on(async {
            tauri_plugin_macos_permissions::check_accessibility_permission().await
        })
    })
    .join()
    {
        Ok(granted) => granted,
        Err(_) => {
            log::warn!("permission check thread panicked; treating permissions as not granted");
            false
        }
    }
}

pub fn should_show_permission_dialog() -> bool {
    !is_autostart_launch() && !required_permissions_granted_sync()
}

#[cfg(test)]
mod tests {
    use super::is_autostart_args;

    #[test]
    fn recognizes_login_launches_without_treating_normal_launches_as_autostart() {
        assert!(is_autostart_args(["chord", "--autostart"]));
        assert!(is_autostart_args(vec![
            "/Applications/Chord.app/Contents/MacOS/chord".to_string(),
            "--autostart".to_string(),
        ]));
        assert!(!is_autostart_args(["chord"]));
        assert!(!is_autostart_args(["chord", "settings"]));
        assert!(!is_autostart_args(["--autostart"]));
    }
}
