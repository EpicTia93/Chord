//! Undoes Chord's Caps key mapping when Chord dies without removing it.
//!
//! `UserKeyMapping` outlives the process that set it, so a crashed Chord would leave
//! Caps Lock dead. The guardian is a launchd agent running Chord's own executable
//! (`__caps-guardian`). launchd keeps it alive exactly while the remap state file
//! exists, including at the next login. It waits on the holder's exit and on changes
//! to the state directory with kqueue, so it reacts immediately and never polls.

use super::caps_remap::{RemapStore, SystemMappingTable};
use anyhow::{Context, Result, bail, ensure};
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The hidden subcommand launchd runs.
pub const COMMAND: &str = "__caps-guardian";

/// Runs the guardian until nothing is left to guard.
pub fn run(state: &Path) -> Result<()> {
    let store = RemapStore::new(state.to_path_buf());
    let directory = state
        .parent()
        .context("the Caps remap state has no directory")?;
    std::fs::create_dir_all(directory)?;
    let queue = Queue::new()?;
    // Atomic writes and removal both change the directory.
    let watched = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EVTONLY)
        .open(directory)
        .context("failed to watch the Caps remap state directory")?;
    queue.add(
        watched.as_raw_fd() as usize,
        libc::EVFILT_VNODE,
        libc::NOTE_WRITE,
    )?;
    loop {
        let Some(state) = store.read()? else {
            log("no Caps key mapping to guard; exiting");
            return Ok(());
        };
        let holder = state.holder();
        // Checking again after registering closes the window where the holder exits
        // (or its PID is recycled) before the watch exists.
        if holder.is_alive()
            && queue
                .add(holder.pid as usize, libc::EVFILT_PROC, libc::NOTE_EXIT)
                .is_ok()
            && holder.is_alive()
        {
            queue.wait()?;
            continue;
        }
        if store.uninstall(&SystemMappingTable, None)? {
            log(&format!(
                "Chord (PID {}) exited without removing its Caps key mapping; removed it",
                holder.pid
            ));
        }
    }
}

fn log(message: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    eprintln!("[unix {}] {message}", now.as_secs());
}

struct Queue(OwnedFd);

impl Queue {
    fn new() -> Result<Self> {
        // SAFETY: no arguments; the result is checked.
        let fd = unsafe { libc::kqueue() };
        ensure!(
            fd >= 0,
            "kqueue failed: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: a fresh descriptor this process owns.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn add(&self, ident: usize, filter: i16, fflags: u32) -> Result<()> {
        let change = libc::kevent {
            ident,
            filter,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no output buffer.
        let result = unsafe {
            libc::kevent(
                self.0.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        ensure!(
            result >= 0,
            "kevent registration failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }

    /// Blocks until any watched event fires.
    fn wait(&self) -> Result<()> {
        let mut event = std::mem::MaybeUninit::<libc::kevent>::zeroed();
        loop {
            // SAFETY: room for exactly one event; no timeout.
            let result = unsafe {
                libc::kevent(
                    self.0.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    event.as_mut_ptr(),
                    1,
                    std::ptr::null(),
                )
            };
            if result >= 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                bail!("kevent failed: {error}");
            }
        }
    }
}

/// The launchd label. Development variants share one agent, so they don't pile up.
pub fn label(identifier: &str) -> String {
    let channel = if crate::app::dev_lockfile_detector::is_development(identifier) {
        "com.leonsilicon.chord.development"
    } else {
        identifier
    };
    format!("{channel}.caps-guardian")
}

/// Installs or updates the agent for this executable. launchd is only asked to reload
/// it when its definition changed, or when it is not loaded.
pub fn ensure_registered(identifier: &str, state: &Path) -> Result<()> {
    let label = label(identifier);
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let agents = PathBuf::from(home).join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents)?;
    let path = agents.join(format!("{label}.plist"));
    let executable = std::env::current_exe().context("failed to locate Chord's executable")?;
    let log = state.with_file_name(format!("{label}.log"));
    let definition = agent_definition(&label, identifier, &executable, state, &log);
    // SAFETY: no arguments.
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let service = format!("{domain}/{label}");
    if std::fs::read_to_string(&path).is_ok_and(|current| current == definition)
        && launchctl(&["print", &service]).is_ok()
    {
        return Ok(());
    }
    let temporary = path.with_extension("plist.tmp");
    std::fs::write(&temporary, &definition)?;
    std::fs::rename(&temporary, &path)?;
    // Not loaded is fine; anything else surfaces through bootstrap.
    let _ = launchctl(&["bootout", &service]);
    launchctl(&["bootstrap", &domain, &path.to_string_lossy()])
        .context("failed to load the Caps guardian")?;
    log::info!("Registered the Caps guardian agent {label}");
    Ok(())
}

fn launchctl(arguments: &[&str]) -> Result<()> {
    let output = Command::new("/bin/launchctl").args(arguments).output()?;
    ensure!(
        output.status.success(),
        "launchctl {} failed ({}): {}",
        arguments.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn agent_definition(
    label: &str,
    identifier: &str,
    executable: &Path,
    state: &Path,
    log: &Path,
) -> String {
    let [label, identifier, executable, state, log] = [
        label,
        identifier,
        &executable.to_string_lossy(),
        &state.to_string_lossy(),
        &log.to_string_lossy(),
    ]
    .map(escape);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>{identifier}</string>
	</array>
	<key>ProgramArguments</key>
	<array>
		<string>{executable}</string>
		<string>{COMMAND}</string>
		<string>--state</string>
		<string>{state}</string>
	</array>
	<key>KeepAlive</key>
	<dict>
		<key>PathState</key>
		<dict>
			<key>{state}</key>
			<true/>
		</dict>
	</dict>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{
        CFArray, CFData, CFDictionary, CFPropertyListCreateWithData, CFRetained, CFString, CFType,
    };

    #[test]
    fn development_variants_share_one_agent() {
        assert_eq!(
            label("com.leonsilicon.chord"),
            "com.leonsilicon.chord.caps-guardian"
        );
        for identifier in [
            "com.leonsilicon.chord.development",
            "com.leonsilicon.chord.development.codex",
        ] {
            assert_eq!(
                label(identifier),
                "com.leonsilicon.chord.development.caps-guardian"
            );
        }
    }

    #[test]
    fn the_agent_runs_while_the_state_exists_and_escapes_paths() {
        let definition = agent_definition(
            "label",
            "com.leonsilicon.chord",
            Path::new("/Apps/A&B <x>.app/chord"),
            Path::new("/data/state.json"),
            Path::new("/data/label.log"),
        );
        assert!(definition.contains("<string>/Apps/A&amp;B &lt;x&gt;.app/chord</string>"));
        assert!(definition.contains("<string>__caps-guardian</string>"));
        assert!(definition.contains(
            "<key>PathState</key>\n\t\t<dict>\n\t\t\t<key>/data/state.json</key>\n\t\t\t<true/>"
        ));
        // launchd must be able to read it, and the escaped path must come back intact.
        let data = CFData::from_bytes(definition.as_bytes());
        // SAFETY: valid data; no format or error output is requested.
        let parsed = unsafe {
            CFPropertyListCreateWithData(
                None,
                Some(&data),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
        .unwrap_or_else(|| panic!("not a property list:\n{definition}"));
        let dictionary = parsed.downcast::<CFDictionary>().unwrap();
        // SAFETY: a launchd job is a dictionary keyed by strings.
        let dictionary =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(dictionary) };
        let arguments = dictionary
            .get(&CFString::from_static_str("ProgramArguments"))
            .unwrap()
            .downcast::<CFArray>()
            .unwrap();
        // SAFETY: ProgramArguments is an array of strings.
        let arguments = unsafe { CFRetained::cast_unchecked::<CFArray<CFString>>(arguments) };
        let arguments: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect();
        assert_eq!(
            arguments,
            [
                "/Apps/A&B <x>.app/chord",
                COMMAND,
                "--state",
                "/data/state.json"
            ]
        );
    }
}
