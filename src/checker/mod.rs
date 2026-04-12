//! System requirements verification and singleton enforcement.
//!
//! Checks that required tools (Docker, buildx, git) are available and
//! ensures only one Dockermint instance runs at a time via a lock file.
//!
//! All Docker commands are routed through the configured socket URI so
//! that remote Docker daemons are checked correctly.

use std::collections::HashMap;
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
/// # Errors
///
/// Returns [`CheckerError::AlreadyRunning`] if another instance holds
/// the lock.
///
/// # Returns
///
/// A [`LockGuard`] that releases the lock on drop.
pub fn ensure_singleton() -> Result<LockGuard, CheckerError> {
    ensure_singleton_at(Path::new(LOCK_FILE))
}

/// Acquire a singleton lock at a custom path (for testing).
///
/// # Arguments
///
/// * `path` - Lock file path
///
/// # Errors
///
/// Returns [`CheckerError::AlreadyRunning`] if another instance holds
/// the lock.
pub fn ensure_singleton_at(path: &Path) -> Result<LockGuard, CheckerError> {
    if path.exists() {
        // Read PID and check if process is still alive
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse::<u32>()
        {
            let proc_path = PathBuf::from(format!("/proc/{pid}"));
            if proc_path.exists() {
                return Err(CheckerError::AlreadyRunning {
                    lock_path: path.to_owned(),
                });
            }
        }
        // Stale lock file -- remove it
        let _ = std::fs::remove_file(path);
    }

    std::fs::write(path, std::process::id().to_string())
        .map_err(|e| CheckerError::CheckFailed(format!("failed to write lock file: {e}")))?;

    Ok(LockGuard {
        path: path.to_owned(),
    })
}

/// RAII guard that removes the lock file on drop.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        let path = std::env::temp_dir().join("dockermint_test.lock");
        {
            let _guard = ensure_singleton_at(&path).expect("should acquire lock");
            assert!(path.exists());
        }
        assert!(!path.exists(), "lock file should be removed");
    }

    #[test]
    fn docker_env_sets_host() {
        let env = docker_env("tcp://10.0.0.1:2376");
        assert_eq!(env["DOCKER_HOST"], "tcp://10.0.0.1:2376");
    }
}
