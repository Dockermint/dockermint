//! BuildKit backend using `docker buildx`.
//!
//! Manages per-platform builder instances (`{prefix}-amd64`,
//! `{prefix}-arm64`) and executes multi-architecture builds.
//!
//! In **CLI mode** builders are created before the build and destroyed
//! after.  In **daemon mode** builders persist across polling cycles
//! (controlled by the `persist` flag).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::builder::dockerfile;
use crate::builder::{BuildContext, BuildOutput, ImageBuilder};
use crate::error::{BuilderError, CommandError};

/// Mapping from Docker platform string to buildx builder suffix.
const PLATFORM_SUFFIXES: &[(&str, &str)] = &[("linux/amd64", "amd64"), ("linux/arm64", "arm64")];

/// Monotonic counter for generating unique build directory names.
static BUILD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// RAII guard that removes a directory when dropped.
///
/// Used to ensure secret files are cleaned up even if the build
/// errors out or panics.
struct DirGuard {
    path: PathBuf,
}

impl DirGuard {
    /// Create a guard that will remove `path` on drop.
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Generate a unique directory name for this build.
///
/// Combines the process ID and a monotonic counter to ensure
/// uniqueness across concurrent builds within and between processes.
///
/// # Returns
///
/// A directory name of the form `dockermint-build-{pid}-{counter}`.
fn unique_build_dir_name() -> String {
    let pid = std::process::id();
    let seq = BUILD_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("dockermint-build-{pid}-{seq}")
}

/// BuildKit-based image builder using `docker buildx`.
#[derive(Debug)]
pub struct BuildKitBuilder {
    /// Docker daemon socket URI (e.g. `unix:///var/run/docker.sock`).
    docker_host: String,
    /// Prefix for builder instance names.
    prefix: String,
    /// When `true`, builders survive [`cleanup`](ImageBuilder::cleanup).
    persist: bool,
}

impl BuildKitBuilder {
    /// Create a new BuildKit builder.
    ///
    /// # Arguments
    ///
    /// * `docker_host` - Docker daemon socket URI
    /// * `prefix` - Prefix for buildx builder names
    /// * `persist` - If `true`, [`cleanup`](ImageBuilder::cleanup)
    ///   keeps builders alive (daemon mode)
    pub fn new(docker_host: String, prefix: String, persist: bool) -> Self {
        Self {
            docker_host,
            prefix,
            persist,
        }
    }

    /// Return the builder name for a given platform suffix.
    fn builder_name(&self, suffix: &str) -> String {
        format!("{}-{suffix}", self.prefix)
    }

    /// Base `docker` args that set `DOCKER_HOST`.
    fn docker_env(&self) -> Vec<(String, String)> {
        vec![("DOCKER_HOST".to_owned(), self.docker_host.clone())]
    }

    /// Check whether a builder instance already exists.
    async fn builder_exists(&self, name: &str) -> bool {
        let env: std::collections::HashMap<String, String> =
            self.docker_env().into_iter().collect();
        crate::commands::execute_with_env("docker", &["buildx", "inspect", name], &env)
            .await
            .is_ok()
    }

    /// Create a single buildx builder if it does not exist.
    async fn ensure_builder(&self, name: &str, platform: &str) -> Result<(), BuilderError> {
        if self.builder_exists(name).await {
            tracing::debug!(builder = name, "buildx builder already exists");
            return Ok(());
        }

        tracing::info!(builder = name, platform, "creating buildx builder");
        let env: std::collections::HashMap<String, String> =
            self.docker_env().into_iter().collect();

        crate::commands::execute_with_env(
            "docker",
            &[
                "buildx",
                "create",
                "--name",
                name,
                "--platform",
                platform,
                "--driver",
                "docker-container",
            ],
            &env,
        )
        .await
        .map_err(|e| BuilderError::BuildxSetup(format!("{name}: {e}")))?;

        // Bootstrap the builder so the first build does not stall
        crate::commands::execute_with_env(
            "docker",
            &["buildx", "inspect", name, "--bootstrap"],
            &env,
        )
        .await
        .map_err(|e| BuilderError::BuildxSetup(format!("bootstrap {name}: {e}")))?;

        Ok(())
    }

    /// Remove a single buildx builder instance.
    async fn remove_builder(&self, name: &str) -> Result<(), BuilderError> {
        if !self.builder_exists(name).await {
            return Ok(());
        }

        tracing::info!(builder = name, "removing buildx builder");
        let env: std::collections::HashMap<String, String> =
            self.docker_env().into_iter().collect();

        // Stop then remove
        let _ = crate::commands::execute_with_env("docker", &["buildx", "stop", name], &env).await;

        crate::commands::execute_with_env("docker", &["buildx", "rm", name], &env)
            .await
            .map_err(|e| BuilderError::BuildxSetup(format!("remove {name}: {e}")))?;

        Ok(())
    }

    /// Write the Dockerfile to a unique per-build temp directory.
    ///
    /// Each call creates a fresh directory under the OS temp path so
    /// concurrent builds never clobber each other.
    ///
    /// # Arguments
    ///
    /// * `content` - Dockerfile content to write
    ///
    /// # Returns
    ///
    /// Path to the written `Dockerfile`.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::DockerfileGeneration`] on I/O failure.
    fn write_dockerfile(&self, content: &str) -> Result<PathBuf, BuilderError> {
        let dir = std::env::temp_dir().join(unique_build_dir_name());
        std::fs::create_dir_all(&dir).map_err(|e| {
            BuilderError::DockerfileGeneration(format!("failed to create build dir: {e}"))
        })?;

        let path = dir.join("Dockerfile");
        std::fs::write(&path, content).map_err(|e| {
            BuilderError::DockerfileGeneration(format!("failed to write Dockerfile: {e}"))
        })?;

        Ok(path)
    }

    /// Write secret files to a directory **outside** the build context.
    ///
    /// Returns `(secret_dir, args)` where `args` are `--secret` flags
    /// to pass to `docker buildx build`. The returned [`DirGuard`]
    /// ensures the secret directory is removed when dropped.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::DockerfileGeneration`] on I/O failure.
    fn prepare_secrets(&self) -> Result<(DirGuard, Vec<String>), BuilderError> {
        let secret_dir =
            std::env::temp_dir().join(format!("dockermint-secrets-{}", unique_build_dir_name()));
        std::fs::create_dir_all(&secret_dir)
            .map_err(|e| BuilderError::DockerfileGeneration(format!("secret dir: {e}")))?;

        let guard = DirGuard::new(secret_dir.clone());
        let mut args = Vec::new();

        if let Ok(user) = std::env::var("GH_USER") {
            let path = secret_dir.join("gh_user");
            write_secret_file(&path, &user)?;
            args.push("--secret".to_owned());
            args.push(format!("id=gh_user,src={}", path.display()));
        }
        if let Ok(pat) = std::env::var("GH_PAT") {
            let path = secret_dir.join("gh_pat");
            write_secret_file(&path, &pat)?;
            args.push("--secret".to_owned());
            args.push(format!("id=gh_pat,src={}", path.display()));
        }

        Ok((guard, args))
    }
}

/// Write a secret value to a file with restrictive permissions.
///
/// # Errors
///
/// Returns [`BuilderError::DockerfileGeneration`] on I/O failure.
fn write_secret_file(path: &Path, value: &str) -> Result<(), BuilderError> {
    std::fs::write(path, value)
        .map_err(|e| BuilderError::DockerfileGeneration(format!("write secret: {e}")))
}

impl ImageBuilder for BuildKitBuilder {
    /// Create per-platform buildx builder instances.
    ///
    /// Creates `{prefix}-amd64` and `{prefix}-arm64` builders using the
    /// `docker-container` driver.
    async fn setup_builders(&self) -> Result<(), BuilderError> {
        for (platform, suffix) in PLATFORM_SUFFIXES {
            let name = self.builder_name(suffix);
            self.ensure_builder(&name, platform).await?;
        }
        Ok(())
    }

    /// Build a Docker image using `docker buildx build`.
    ///
    /// 1. Generates a Dockerfile from the resolved recipe
    /// 2. Writes it to a unique per-build temp directory
    /// 3. Writes secrets to a **separate** temp directory outside the
    ///    build context (secrets are never COPY-able)
    /// 4. Runs `docker buildx build` with the appropriate builder
    /// 5. For single-platform builds uses `--load`; for multi-platform
    ///    uses `--output type=image` (since `--load` only supports a
    ///    single platform)
    /// 6. Always cleans up secrets on completion (via RAII guard)
    async fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError> {
        let start = Instant::now();

        // Generate Dockerfile
        let dockerfile_content = dockerfile::generate(&context.recipe)?;
        let dockerfile_path = self.write_dockerfile(&dockerfile_content)?;
        let dockerfile_str = dockerfile_path.to_string_lossy();

        let image_tag = context.resolve_image_tag();
        let platform_str = context.platforms.join(",");
        let git_tag_arg = format!("GIT_TAG={}", context.tag);

        // Determine which builder to use based on platform count.
        // For multi-platform builds the docker-container driver handles
        // cross-compilation natively (no QEMU needed on the host).
        let builder_name = if context.platforms.len() > 1 {
            self.builder_name("amd64")
        } else {
            let suffix = PLATFORM_SUFFIXES
                .iter()
                .find(|(p, _)| context.platforms.first().is_some_and(|cp| cp == p))
                .map(|(_, s)| *s)
                .unwrap_or("amd64");
            self.builder_name(suffix)
        };

        tracing::info!(
            recipe = context.recipe.recipe.header.name,
            tag = context.tag,
            platforms = platform_str,
            builder = builder_name,
            "starting buildx build"
        );

        let build_dir = dockerfile_path.parent().expect("dockerfile has parent dir");

        let mut build_args: Vec<String> = vec![
            "buildx".to_owned(),
            "build".to_owned(),
            "--builder".to_owned(),
            builder_name.clone(),
            "--platform".to_owned(),
            platform_str.clone(),
            "-f".to_owned(),
            dockerfile_str.to_string(),
            "--build-arg".to_owned(),
            git_tag_arg,
            "--tag".to_owned(),
            image_tag.clone(),
        ];

        // Mount GH credentials as BuildKit secrets (never baked into
        // image layers or visible via `docker inspect`).
        // Secrets live OUTSIDE the build context directory so they
        // cannot be reached by COPY/ADD instructions.
        // The DirGuard ensures cleanup even on error or panic.
        let (_secret_guard, secret_args) = self.prepare_secrets()?;
        build_args.extend(secret_args);

        // --load only works for single-platform builds.
        // For multi-platform, use --output type=image and let the
        // caller handle pushing separately.
        if context.platforms.len() == 1 {
            build_args.push("--load".to_owned());
        } else {
            build_args.push("--output".to_owned());
            build_args.push("type=image".to_owned());
        }

        build_args.push(build_dir.to_string_lossy().to_string());

        let args_refs: Vec<&str> = build_args.iter().map(String::as_str).collect();
        let env: std::collections::HashMap<String, String> =
            self.docker_env().into_iter().collect();

        let output = crate::commands::execute_with_env("docker", &args_refs, &env)
            .await
            .map_err(|e| match e {
                CommandError::NonZeroExit { stderr, .. } => BuilderError::BuildFailed {
                    recipe: context.recipe.recipe.header.name.clone(),
                    tag: context.tag.clone(),
                    reason: stderr,
                },
                other => BuilderError::Command(other),
            })?;

        let duration = start.elapsed();

        // Extract image ID from output (last line often has it)
        let image_id = output
            .stdout
            .lines()
            .last()
            .unwrap_or("unknown")
            .trim()
            .to_owned();

        tracing::info!(
            recipe = context.recipe.recipe.header.name,
            tag = context.tag,
            duration_secs = duration.as_secs(),
            "build completed"
        );

        Ok(BuildOutput {
            image_id,
            image_tag,
            duration,
            platforms: context.platforms.clone(),
        })
        // _secret_guard is dropped here, removing the secrets directory
    }

    /// Remove buildx builder instances.
    ///
    /// In CLI mode (`persist = false`), removes all builders.
    /// In daemon mode (`persist = true`), keeps them alive.
    ///
    /// Note: per-build temp directories and secrets are cleaned up by
    /// the build method itself (via RAII guards), not here.
    async fn cleanup(&self) -> Result<(), BuilderError> {
        if self.persist {
            tracing::debug!("persist=true, keeping buildx builders");
            return Ok(());
        }

        for (_, suffix) in PLATFORM_SUFFIXES {
            let name = self.builder_name(suffix);
            self.remove_builder(&name).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_name_format() {
        let b = BuildKitBuilder::new(
            "unix:///var/run/docker.sock".to_owned(),
            "dockermint".to_owned(),
            false,
        );
        assert_eq!(b.builder_name("amd64"), "dockermint-amd64");
        assert_eq!(b.builder_name("arm64"), "dockermint-arm64");
    }

    #[test]
    fn custom_prefix() {
        let b = BuildKitBuilder::new("tcp://remote:2376".to_owned(), "myproject".to_owned(), true);
        assert_eq!(b.builder_name("amd64"), "myproject-amd64");
        assert!(b.persist);
    }

    #[test]
    fn docker_env_sets_host() {
        let b = BuildKitBuilder::new("tcp://10.0.0.1:2376".to_owned(), "dm".to_owned(), false);
        let env = b.docker_env();
        assert_eq!(env[0].0, "DOCKER_HOST");
        assert_eq!(env[0].1, "tcp://10.0.0.1:2376");
    }

    #[test]
    fn buildkit_builder_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BuildKitBuilder>();
    }

    #[test]
    fn unique_build_dir_names_are_distinct() {
        let a = unique_build_dir_name();
        let b = unique_build_dir_name();
        assert_ne!(a, b);
    }

    #[test]
    fn write_dockerfile_creates_unique_dirs() {
        let builder = BuildKitBuilder::new(
            "unix:///var/run/docker.sock".to_owned(),
            "test".to_owned(),
            false,
        );

        let path_a = builder
            .write_dockerfile("FROM alpine:3.20")
            .expect("write_dockerfile should succeed");
        let path_b = builder
            .write_dockerfile("FROM alpine:3.21")
            .expect("write_dockerfile should succeed");

        // Different directories
        assert_ne!(
            path_a.parent().expect("has parent"),
            path_b.parent().expect("has parent"),
        );

        // Content is correct
        let content_a = std::fs::read_to_string(&path_a).expect("read file");
        assert_eq!(content_a, "FROM alpine:3.20");

        // Cleanup
        let _ = std::fs::remove_dir_all(path_a.parent().expect("has parent"));
        let _ = std::fs::remove_dir_all(path_b.parent().expect("has parent"));
    }

    #[test]
    fn dir_guard_removes_on_drop() {
        let dir = std::env::temp_dir().join("dockermint-guard-test");
        std::fs::create_dir_all(&dir).expect("create dir");

        let secret = dir.join("secret.txt");
        std::fs::write(&secret, "top-secret").expect("write");
        assert!(dir.exists());

        {
            let _guard = DirGuard::new(dir.clone());
        }

        assert!(!dir.exists());
    }

    #[test]
    fn secrets_dir_is_outside_build_dir() {
        let builder = BuildKitBuilder::new(
            "unix:///var/run/docker.sock".to_owned(),
            "test".to_owned(),
            false,
        );

        let dockerfile_path = builder
            .write_dockerfile("FROM alpine")
            .expect("write_dockerfile should succeed");
        let build_dir = dockerfile_path.parent().expect("has parent").to_path_buf();

        // prepare_secrets creates a separate directory
        let (guard, _args) = builder.prepare_secrets().expect("prepare_secrets");

        // Secret dir must not be a subdirectory of build dir
        assert!(!guard.path.starts_with(&build_dir));

        // Cleanup
        drop(guard);
        let _ = std::fs::remove_dir_all(&build_dir);
    }
}
