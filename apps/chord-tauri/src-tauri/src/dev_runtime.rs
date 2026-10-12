//! Leases keep signed development snapshots alive for apps and detached CLI workers.
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

fn snapshot_directory() -> Result<Option<PathBuf>> {
    let executable = std::env::current_exe()?;
    Ok(snapshot_directory_for(&executable))
}

fn snapshot_directory_for(executable: &std::path::Path) -> Option<PathBuf> {
    let Some(directory) = executable
        .parent() // MacOS
        .and_then(|p| p.parent()) // Contents
        .and_then(|p| p.parent()) // .app
        .and_then(|p| p.parent())
    else {
        return None;
    };
    if executable.parent().and_then(|p| p.file_name()) != Some("MacOS".as_ref())
        || !directory.join("runtime.lock").is_file()
    {
        return None;
    }
    Some(directory.to_owned())
}

pub fn mark_replaced(pid: libc::pid_t) -> Result<()> {
    let mut path = [0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // The PID was authenticated using the live single-instance socket.
    let length = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if length <= 0 {
        return Ok(());
    }
    use std::os::unix::ffi::OsStrExt;
    let end = path
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(path.len());
    let executable = std::path::Path::new(std::ffi::OsStr::from_bytes(&path[..end]));
    if let Some(directory) = snapshot_directory_for(executable) {
        std::fs::write(directory.join("handoff"), "")
            .context("mark development runtime handoff")?;
        // Older runners only read app.exit; a requested handoff is successful.
        std::fs::write(directory.join("app.exit"), "0")
            .context("publish development runtime handoff status")?;
    }
    Ok(())
}

pub fn publish_exit(status: i32) -> Result<()> {
    if let Some(directory) = snapshot_directory()? {
        let pid = std::fs::read_to_string(directory.join("app.pid"))?;
        if pid.trim() == std::process::id().to_string() {
            std::fs::write(directory.join("app.exit"), status.to_string())?;
        }
    }
    Ok(())
}

pub fn initialize_stdio() -> Result<()> {
    let Some(directory) = snapshot_directory()? else {
        return Ok(());
    };
    if directory.join("app.pid").exists() {
        return Ok(());
    }
    let Some(requested) = std::env::var_os("CHORD_DEV_STDIO_DIRECTORY") else {
        return Ok(());
    };
    if std::fs::canonicalize(requested)? != directory {
        anyhow::bail!("development stdio directory does not match this snapshot");
    }
    for (name, fd) in [
        ("stdout", libc::STDOUT_FILENO),
        ("stderr", libc::STDERR_FILENO),
    ] {
        let pipe = OpenOptions::new().write(true).open(directory.join(name))?;
        if unsafe { libc::dup2(pipe.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error()).context("connect development log stream");
        }
    }
    Ok(())
}

pub struct RuntimeLease {
    _file: File,
    exit_file: Option<PathBuf>,
}

impl Drop for RuntimeLease {
    fn drop(&mut self) {
        if let Some(path) = &self.exit_file {
            // SIGKILL and abort skip Drop; the runner treats a missing result as
            // failure unless it deliberately stopped the application.
            let status = if std::thread::panicking() { "101" } else { "0" };
            // A Tauri exit callback may already have published a nonzero code.
            if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(path) {
                let _ = write!(file, "{status}");
            }
        }
    }
}

pub fn acquire(app_launch: bool) -> Result<Option<RuntimeLease>> {
    let Some(directory) = snapshot_directory()? else {
        return Ok(None);
    };
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("runtime.lock"))
        .context("open development runtime lease")?;
    lease
        .lock_shared()
        .context("lock development runtime snapshot")?;
    // Keep the lease across fork/exec so a detached CLI child is protected even
    // in the interval before its own main() acquires a shared lock.
    let flags = unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        return Err(std::io::Error::last_os_error()).context("inherit development runtime lease");
    }
    let mut owner = false;
    if app_launch {
        // Only the runner's first app writes this file. A later __app-chord
        // invocation must not replace the PID used for signal forwarding.
        let temporary = directory.join(format!("pid-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        writeln!(file, "{}", std::process::id())?;
        drop(file);
        // Linking a fully written temporary file publishes it atomically while
        // retaining create_new semantics for concurrent app-chord invocations.
        let published = std::fs::hard_link(&temporary, directory.join("app.pid"));
        let _ = std::fs::remove_file(temporary);
        match published {
            Ok(()) => owner = true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("publish development app PID"),
        }
    }
    Ok(Some(RuntimeLease {
        _file: lease,
        exit_file: owner.then(|| directory.join("app.exit")),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn lease_fixture() {
        let Ok(release) = std::env::var("CHORD_TEST_RUNTIME_RELEASE") else {
            return;
        };
        let _lease = acquire(true)
            .unwrap()
            .expect("fixture must run from a snapshot");
        while !std::path::Path::new(&release).exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_cli_worker_holds_its_snapshot_until_exit() {
        let directory =
            std::env::temp_dir().join(format!("chord-runtime-{}", uuid::Uuid::new_v4()));
        let executable = directory.join("Chord.app/Contents/MacOS/chord");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let lock = File::create(directory.join("runtime.lock")).unwrap();
        let release = directory.join("release");
        let mut child = Command::new(executable)
            .args(["--exact", "dev_runtime::tests::lease_fixture"])
            .env("CHORD_TEST_RUNTIME_RELEASE", &release)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !directory.join("app.pid").exists() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("fixture did not acquire its runtime lease");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            lock.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        std::fs::write(release, "").unwrap();
        assert!(child.wait().unwrap().success());
        lock.try_lock().unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.join("app.exit")).unwrap(),
            "0"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

pub fn cli_executable() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let Some(directory) = snapshot_directory()? else {
        return Ok(executable);
    };
    let root = directory
        .parent()
        .context("development snapshot has no parent")?;
    // The runner updates this symlink atomically after a successful build and
    // keeps its generation. Installed CLI links survive restarts and collection.
    Ok(root.join("current/Contents/MacOS/chord"))
}
