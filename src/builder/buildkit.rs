//! BuildKit backend using `docker buildx`.
//!
//! Manages per-platform builder instances (`{prefix}-amd64`,
//! `{prefix}-arm64`) and executes multi-architecture builds.
//!
//! In **CLI mode** builders are created before the build and destroyed
//! after.  In **daemon mode** builders persist across polling cycles
//! (controlled by the `persist` flag).

use std::path::PathBuf;
use std::time::Instant;

use crate::builder::dockerfile;
use crate::builder::{BuildContext, BuildOutput, ImageBuilder};
use crate::error::{BuilderError, CommandError};

/// Mapping from Docker platform string to buildx builder suffix.
const PLATFORM_SUFFIXES: &[(&str, &str)] = &[("linux/amd64", "amd64"), ("linux/arm64", "arm64")];

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
        let env = self.docker_env();
        let env_refs: std::collections::HashMap<String, String> = env.into_iter().collect();
        crate::commands::execute_with_env("docker", &["buildx", "inspect", name], &env_refs)
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

    /// Write the Dockerfile to a temp directory and return its path.
    fn write_dockerfile(&self, content: &str) -> Result<PathBuf, BuilderError> {
        let dir = std::env::temp_dir().join("dockermint-build");
        std::fs::create_dir_all(&dir).map_err(|e| {
            BuilderError::DockerfileGeneration(format!("failed to create build dir: {e}"))
        })?;

        let path = dir.join("Dockerfile");
        std::fs::write(&path, content).map_err(|e| {
            BuilderError::DockerfileGeneration(format!("failed to write Dockerfile: {e}"))
        })?;

        Ok(path)
    }
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
    /// 2. Writes it to a temp directory
    /// 3. Runs `docker buildx build` with the appropriate builder
    /// 4. Returns the build output
    async fn build(&self, context: &BuildContext) -> Result<BuildOutput, BuilderError> {
        let start = Instant::now();

        // Generate Dockerfile
        let dockerfile_content = dockerfile::generate(&context.recipe)?;
        let dockerfile_path = self.write_dockerfile(&dockerfile_content)?;
        let dockerfile_str = dockerfile_path.to_string_lossy();

        let image_tag = context.resolve_image_tag();
        let platform_str = context.platforms.join(",");
        let git_tag_arg = format!("GIT_TAG={}", context.tag);

        // Determine which builder to use based on platform count
        // For multi-platform, we need a builder that supports all
        // For single platform, pick the matching one
        let builder_name = if context.platforms.len() > 1 {
            // Multi-platform: use the amd64 builder (it can build both
            // via QEMU when using docker-container driver)
            self.builder_name("amd64")
        } else {
            // Single platform: match the suffix
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
        let build_dir = dockerfile_path.parent().expect("dockerfile has parent dir");
        let secret_dir = build_dir.join("secrets");
        std::fs::create_dir_all(&secret_dir)
            .map_err(|e| BuilderError::DockerfileGeneration(format!("secret dir: {e}")))?;

        if let Ok(user) = std::env::var("GH_USER") {
            let path = secret_dir.join("gh_user");
            std::fs::write(&path, &user)
                .map_err(|e| BuilderError::DockerfileGeneration(format!("write secret: {e}")))?;
            build_args.push("--secret".to_owned());
            build_args.push(format!("id=gh_user,src={}", path.display()));
        }
        if let Ok(pat) = std::env::var("GH_PAT") {
            let path = secret_dir.join("gh_pat");
            std::fs::write(&path, &pat)
                .map_err(|e| BuilderError::DockerfileGeneration(format!("write secret: {e}")))?;
            build_args.push("--secret".to_owned());
            build_args.push(format!("id=gh_pat,src={}", path.display()));
        }

        // --load for single-platform, --push handled by caller
        build_args.push("--load".to_owned());

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
    }

    /// Remove buildx builder instances.
    ///
    /// In CLI mode (`persist = false`), removes all builders.
    /// In daemon mode (`persist = true`), keeps them alive.
    async fn cleanup(&self) -> Result<(), BuilderError> {
        if self.persist {
            tracing::debug!("persist=true, keeping buildx builders");
            return Ok(());
        }

        for (_, suffix) in PLATFORM_SUFFIXES {
            let name = self.builder_name(suffix);
            self.remove_builder(&name).await?;
        }

        // Clean up temp build directory
        let dir = std::env::temp_dir().join("dockermint-build");
        let _ = std::fs::remove_dir_all(dir);

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
}
