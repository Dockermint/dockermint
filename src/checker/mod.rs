//! System requirements verification and singleton enforcement.
//!
//! Checks that required tools (Docker, buildx, git) are available and
//! ensures only one Dockermint instance runs at a time via a lock file.
//!
//! Lock acquisition uses atomic `create_new` to eliminate TOCTOU races.
//! Process liveness is checked with `kill -0` for cross-platform support
//! (Linux and macOS).
//!
//! All Docker commands are routed through the configured socket URI so
//! that remote Docker daemons are checked correctly.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::CheckerError;

/// Default lock file path for singleton enforcement.
const LOCK_FILE: &str = "/tmp/dockermint.lock";

/// Results of the system requirements check.
#[derive(Debug, Clone)]
pub struct SystemRequirements {
    /// Whether `docker` is available and responsive.
    pub docker: bool,
    /// Whether `docker buildx` is available.
    pub buildx: bool,
    /// Whether `git` is available.
    pub git: bool,
}

/// Verify that all required system tools are installed and accessible.
///
/// Docker commands are executed against the provided socket URI so that
/// remote daemons and custom socket paths are validated.
///
/// # Arguments
///
/// * `docker_socket_uri` - Docker daemon socket URI from
///   [`DockerConfig::socket_uri`](crate::config::types::DockerConfig)
///
/// # Returns
///
/// [`SystemRequirements`] indicating which tools are available.
///
/// # Errors
///
/// Returns [`CheckerError::MissingTool`] for the first required tool
/// that is missing or unreachable.
pub async fn verify_requirements(
    docker_socket_uri: &str,
) -> Result<SystemRequirements, CheckerError> {
    let docker_env = docker_env(docker_socket_uri);

    let docker = check_tool_with_env("docker", &["version"], &docker_env).await;
    let buildx = check_tool_with_env("docker", &["buildx", "version"], &docker_env).await;
    let git = check_tool("git", &["--version"]).await;

    if !docker {
        return Err(CheckerError::MissingTool {
            tool: format!("docker (via {docker_socket_uri})"),
        });
    }
    if !buildx {
        return Err(CheckerError::MissingTool {
            tool: format!("docker buildx (via {docker_socket_uri})"),
        });
    }
    if !git {
        return Err(CheckerError::MissingTool {
            tool: "git".to_owned(),
        });
    }

    Ok(SystemRequirements {
        docker,
        buildx,
        git,
    })
}

/// Acquire a file-based lock to enforce single-instance execution.
///
/// Uses the default lock path ([`LOCK_FILE`]).
///
/// # Errors
///
/// Returns [`CheckerError::AlreadyRunning`] if another live instance
/// holds the lock.
///
/// # Returns
///
/// A [`LockGuard`] that releases the lock on drop.
pub fn ensure_singleton() -> Result<LockGuard, CheckerError> {
    ensure_singleton_at(Path::new(LOCK_FILE))
}

/// Acquire a singleton lock at a custom path.
///
/// The lock is created atomically with `O_CREAT | O_EXCL` semantics
/// (`create_new`), eliminating TOCTOU races. If the file already
/// exists, the recorded PID is read and checked for liveness via
/// `kill -0` (cross-platform on Linux and macOS). A stale lock is
/// removed and creation is retried once.
///
/// The returned [`LockGuard`] holds the open file handle for the
/// duration of the lock and removes the file on drop only if the
/// current process still owns it.
///
/// # Arguments
///
/// * `path` - Lock file path
///
/// # Errors
///
/// Returns [`CheckerError::AlreadyRunning`] if another live instance
/// holds the lock.
///
/// Returns [`CheckerError::CheckFailed`] if the lock file cannot be
/// created due to an I/O error other than `AlreadyExists`.
pub fn ensure_singleton_at(path: &Path) -> Result<LockGuard, CheckerError> {
    match try_create_lock(path) {
        Ok(guard) => Ok(guard),
        Err(CheckerError::AlreadyRunning { .. }) => {
            // File exists -- check if the owning process is alive.
            if is_lock_held_by_live_process(path) {
                return Err(CheckerError::AlreadyRunning {
                    lock_path: path.to_owned(),
                });
            }
            // Stale lock: remove and retry once.
            let _ = std::fs::remove_file(path);
            try_create_lock(path)
        },
        Err(e) => Err(e),
    }
}

/// Attempt atomic lock file creation and write our PID into it.
///
/// # Arguments
///
/// * `path` - Lock file path
///
/// # Errors
///
/// Returns [`CheckerError::AlreadyRunning`] if the file already exists.
///
/// Returns [`CheckerError::CheckFailed`] on any other I/O error.
fn try_create_lock(path: &Path) -> Result<LockGuard, CheckerError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                CheckerError::AlreadyRunning {
                    lock_path: path.to_owned(),
                }
            } else {
                CheckerError::CheckFailed(format!("failed to create lock file: {e}"))
            }
        })?;

    let pid = std::process::id();
    write!(file, "{pid}").map_err(|e| {
        // Clean up the empty file on write failure.
        let _ = std::fs::remove_file(path);
        CheckerError::CheckFailed(format!("failed to write PID to lock file: {e}"))
    })?;

    Ok(LockGuard {
        path: path.to_owned(),
        _file: file,
        pid,
    })
}

/// Check whether the PID recorded in an existing lock file belongs to a
/// running process.
///
/// Uses `kill -0 <pid>` which works on both Linux and macOS (no `/proc`
/// dependency). Returns `false` if the file cannot be read or parsed,
/// allowing the caller to treat the lock as stale.
fn is_lock_held_by_live_process(path: &Path) -> bool {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid_str = contents.trim();
    if pid_str.parse::<u32>().is_err() {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", pid_str])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// RAII guard that removes the lock file on drop.
///
/// Holds the open file handle to keep the lock active. On drop the file
/// is removed only if the current process PID still matches the one
/// recorded at creation time, preventing accidental removal of a lock
/// acquired by another instance.
#[derive(Debug)]
pub struct LockGuard {
    /// Path to the lock file.
    path: PathBuf,
    /// Open file handle -- kept alive for the duration of the lock.
    _file: File,
    /// PID that was written when the lock was acquired.
    pid: u32,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Only remove if the file still contains our PID.
        let dominated = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|c| c.trim().parse::<u32>().ok())
            .is_some_and(|file_pid| file_pid == self.pid);
        if dominated {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────

/// Build a `DOCKER_HOST` environment map from a socket URI.
fn docker_env(socket_uri: &str) -> HashMap<String, String> {
    let mut env = HashMap::with_capacity(1);
    env.insert("DOCKER_HOST".to_owned(), socket_uri.to_owned());
    env
}

/// Check whether a tool is available (no extra env).
async fn check_tool(cmd: &str, args: &[&str]) -> bool {
    crate::commands::execute_unchecked(cmd, args)
        .await
        .is_ok_and(|out| out.status.success())
}

/// Check whether a tool is available with extra environment variables.
async fn check_tool_with_env(cmd: &str, args: &[&str], env: &HashMap<String, String>) -> bool {
    crate::commands::execute_unchecked_with_env(cmd, args, env)
        .await
        .is_ok_and(|out| out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_guard_removes_file_on_drop() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("dockermint_test.lock");
        {
            let _guard = ensure_singleton_at(&path).expect("should acquire lock");
            assert!(path.exists());
        }
        assert!(!path.exists(), "lock file should be removed");
    }

    #[test]
    fn lock_file_contains_current_pid() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("pid_check.lock");
        let _guard = ensure_singleton_at(&path).expect("should acquire lock");

        let contents = std::fs::read_to_string(&path).expect("should read lock file");
        let stored_pid: u32 = contents
            .trim()
            .parse()
            .expect("lock file should contain a numeric PID");
        assert_eq!(stored_pid, std::process::id());
    }

    #[test]
    fn double_acquire_fails() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("double.lock");
        let _guard = ensure_singleton_at(&path).expect("should acquire lock");

        let err = ensure_singleton_at(&path).expect_err("second acquire should fail");
        assert!(
            matches!(err, CheckerError::AlreadyRunning { .. }),
            "expected AlreadyRunning, got: {err:?}"
        );
    }

    #[test]
    fn stale_lock_from_dead_process_is_reclaimed() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("stale.lock");

        // Write a lock file with a PID that does not exist.
        // PID 4_000_000 is above the typical Linux max (4_194_304 default
        // is the ceiling, but most systems never reach it).
        std::fs::write(&path, "4000000").expect("should write stale lock");

        let guard = ensure_singleton_at(&path).expect("should reclaim stale lock");
        let contents = std::fs::read_to_string(&path).expect("should read lock file");
        assert_eq!(
            contents.trim(),
            std::process::id().to_string(),
            "lock should now contain our PID"
        );
        drop(guard);
    }

    #[test]
    fn corrupt_lock_file_is_reclaimed() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("corrupt.lock");

        std::fs::write(&path, "not-a-pid").expect("should write corrupt lock");

        let _guard = ensure_singleton_at(&path).expect("should reclaim corrupt lock");
        assert!(path.exists());
    }

    #[test]
    fn drop_does_not_remove_file_with_foreign_pid() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("foreign.lock");
        {
            let _guard = ensure_singleton_at(&path).expect("should acquire lock");
            // Overwrite the PID with a foreign value before drop.
            std::fs::write(&path, "999999").expect("should overwrite lock");
        }
        // Drop should NOT have removed the file because PID mismatches.
        assert!(
            path.exists(),
            "lock file should remain when PID does not match"
        );
    }

    #[test]
    fn docker_env_sets_host() {
        let env = docker_env("tcp://10.0.0.1:2376");
        assert_eq!(env["DOCKER_HOST"], "tcp://10.0.0.1:2376");
    }

    // -- additional tests for mutation coverage --

    #[test]
    fn stale_lock_with_empty_file_is_reclaimed() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("empty.lock");
        std::fs::write(&path, "").expect("should write empty lock");

        let _guard = ensure_singleton_at(&path).expect("should reclaim empty lock");
        let contents = std::fs::read_to_string(&path).expect("should read lock file");
        assert_eq!(contents.trim(), std::process::id().to_string());
    }

    #[test]
    fn stale_lock_with_whitespace_only_is_reclaimed() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("whitespace.lock");
        std::fs::write(&path, "   \n  ").expect("should write whitespace");

        let _guard = ensure_singleton_at(&path).expect("should reclaim whitespace lock");
        assert!(path.exists());
    }

    #[test]
    fn is_lock_held_by_live_process_returns_false_for_nonexistent_file() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("nonexistent.lock");
        assert!(!is_lock_held_by_live_process(&path));
    }

    #[test]
    fn is_lock_held_by_live_process_returns_false_for_corrupt_content() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("corrupt_live.lock");
        std::fs::write(&path, "abc-not-a-pid").expect("write");
        assert!(!is_lock_held_by_live_process(&path));
    }

    #[test]
    fn is_lock_held_by_live_process_returns_false_for_dead_pid() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("dead_pid.lock");
        std::fs::write(&path, "4000000").expect("write");
        assert!(!is_lock_held_by_live_process(&path));
    }

    #[test]
    fn is_lock_held_by_live_process_returns_true_for_current_pid() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("self_pid.lock");
        std::fs::write(&path, std::process::id().to_string()).expect("write");
        assert!(is_lock_held_by_live_process(&path));
    }

    #[test]
    fn try_create_lock_fails_when_file_exists() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("exists.lock");
        std::fs::write(&path, "12345").expect("create file");

        let err = try_create_lock(&path).expect_err("should fail");
        assert!(
            matches!(err, CheckerError::AlreadyRunning { .. }),
            "expected AlreadyRunning, got: {err:?}"
        );
    }

    #[test]
    fn try_create_lock_succeeds_on_fresh_path() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("fresh.lock");

        let guard = try_create_lock(&path).expect("should succeed");
        assert!(path.exists());
        assert_eq!(guard.pid, std::process::id());
        drop(guard);
    }

    #[test]
    fn docker_env_has_exactly_one_entry() {
        let env = docker_env("unix:///var/run/docker.sock");
        assert_eq!(env.len(), 1);
        assert!(env.contains_key("DOCKER_HOST"));
    }

    #[test]
    fn lock_guard_pid_matches_process_id() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("pid_field.lock");
        let guard = ensure_singleton_at(&path).expect("should acquire lock");
        assert_eq!(guard.pid, std::process::id());
        drop(guard);
    }

    #[test]
    fn double_acquire_error_contains_lock_path() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("path_check.lock");
        let _guard = ensure_singleton_at(&path).expect("acquire");

        let err = ensure_singleton_at(&path).expect_err("second");
        match err {
            CheckerError::AlreadyRunning { lock_path } => {
                assert_eq!(lock_path, path);
            },
            other => panic!("expected AlreadyRunning, got: {other:?}"),
        }
    }

    #[test]
    fn system_requirements_debug() {
        let req = SystemRequirements {
            docker: true,
            buildx: true,
            git: false,
        };
        let dbg = format!("{req:?}");
        assert!(dbg.contains("docker: true"));
        assert!(dbg.contains("git: false"));
    }
}
