//! Shell command execution utilities.
//!
//! Wraps [`tokio::process::Command`] with structured output capture and
//! error propagation.

use std::collections::HashMap;
use std::process::ExitStatus;

use crate::error::CommandError;

/// Captured output from a completed shell command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Standard output (UTF-8 lossy).
    pub stdout: String,
    /// Standard error (UTF-8 lossy).
    pub stderr: String,
    /// Process exit status.
    pub status: ExitStatus,
}

/// Execute a command and capture its output.
///
/// # Arguments
///
/// * `cmd` - Program name or path
/// * `args` - Command-line arguments
///
/// # Returns
///
/// [`CommandOutput`] with captured stdout, stderr, and exit status.
///
/// # Errors
///
/// - [`CommandError::Spawn`] if the process cannot be started.
/// - [`CommandError::NonZeroExit`] if the process exits with non-zero
///   status.
pub async fn execute(cmd: &str, args: &[&str]) -> Result<CommandOutput, CommandError> {
    execute_with_env(cmd, args, &HashMap::new()).await
}

/// Execute a command with additional environment variables.
///
/// # Arguments
///
/// * `cmd` - Program name or path
/// * `args` - Command-line arguments
/// * `env` - Additional environment variables to set
///
/// # Returns
///
/// [`CommandOutput`] with captured stdout, stderr, and exit status.
///
/// # Errors
///
/// - [`CommandError::Spawn`] if the process cannot be started.
/// - [`CommandError::NonZeroExit`] if the process exits with non-zero
///   status.
pub async fn execute_with_env(
    cmd: &str,
    args: &[&str],
    env: &HashMap<String, String>,
) -> Result<CommandOutput, CommandError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .envs(env)
        .output()
        .await
        .map_err(|e| CommandError::Spawn {
            command: cmd.to_owned(),
            source: e,
        })?;

    let result = CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status,
    };

    if !result.status.success() {
        return Err(CommandError::NonZeroExit {
            command: format!("{cmd} {}", args.join(" ")),
            status: result.status.code().unwrap_or(-1),
            stderr: result.stderr.clone(),
        });
    }

    Ok(result)
}

/// Execute a command, returning output even on non-zero exit
/// (does not treat non-zero as an error).
///
/// # Arguments
///
/// * `cmd` - Program name or path
/// * `args` - Command-line arguments
///
/// # Returns
///
/// [`CommandOutput`] regardless of exit code.
///
/// # Errors
///
/// - [`CommandError::Spawn`] only if the process cannot be started.
pub async fn execute_unchecked(cmd: &str, args: &[&str]) -> Result<CommandOutput, CommandError> {
    execute_unchecked_with_env(cmd, args, &HashMap::new()).await
}

/// Execute a command with additional environment variables, returning
/// output even on non-zero exit.
///
/// # Arguments
///
/// * `cmd` - Program name or path
/// * `args` - Command-line arguments
/// * `env` - Additional environment variables to set
///
/// # Returns
///
/// [`CommandOutput`] regardless of exit code.
///
/// # Errors
///
/// - [`CommandError::Spawn`] only if the process cannot be started.
pub async fn execute_unchecked_with_env(
    cmd: &str,
    args: &[&str],
    env: &HashMap<String, String>,
) -> Result<CommandOutput, CommandError> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .envs(env)
        .output()
        .await
        .map_err(|e| CommandError::Spawn {
            command: cmd.to_owned(),
            source: e,
        })?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_echo() {
        let out = execute("echo", &["hello"])
            .await
            .expect("echo should succeed");
        assert_eq!(out.stdout.trim(), "hello");
        assert!(out.status.success());
    }

    #[tokio::test]
    async fn execute_nonexistent_command_returns_spawn_error() {
        let err = execute("__nonexistent_cmd__", &[]).await.unwrap_err();
        assert!(
            matches!(err, CommandError::Spawn { .. }),
            "expected Spawn error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn execute_false_returns_non_zero() {
        let err = execute("false", &[]).await.unwrap_err();
        assert!(
            matches!(err, CommandError::NonZeroExit { .. }),
            "expected NonZeroExit, got: {err:?}"
        );
    }
}
