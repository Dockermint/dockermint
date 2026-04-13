//! Dockermint -- CI/CD pipeline for Cosmos SDK blockchain Docker images.
//!
//! # Architecture
//!
//! Dockermint is organized into feature-gated modules.  Each replaceable
//! component defines a trait, and the active implementation is selected
//! at compile time via Cargo features.
//!
//! | Module    | Trait                | Default feature | Default impl          |
//! |-----------|----------------------|-----------------|-----------------------|
//! | Database  | [`saver::Database`]  | `redb`          | [`saver::redb`]       |
//! | Notifier  | [`notifier::Notifier`] | `telegram`   | [`notifier::telegram`]|
//! | VCS       | [`scrapper::VersionControlSystem`] | `github` | [`scrapper::github`] |
//! | Registry  | [`push::RegistryClient`] | `oci`      | [`push::oci`]         |
//! | Builder   | [`builder::ImageBuilder`] | `buildkit` | [`builder::buildkit`] |
//! | Metrics   | [`metrics::MetricsCollector`] | `prometheus` | [`metrics::prometheus`] |
//!
//! # Modes
//!
//! - **CLI**: one-shot build from a recipe + tag.
//! - **Daemon**: continuous polling for new VCS releases (optionally
//!   with RPC server via `--rpc`).

// ── Modules ──────────────────────────────────────────────────────────

/// Central error types for all modules.
pub mod error;

/// Shell command execution.
pub mod commands;

/// Configuration loading and types.
pub mod config;

/// Recipe parsing, flavor resolution, and validation.
pub mod recipe;

/// Structured logging with rotation.
pub mod logger;

/// System requirements verification and singleton lock.
pub mod checker;

/// Docker image building: templates, Dockerfile generation, buildx.
pub mod builder;

/// VCS integration for fetching releases.
pub mod scrapper;

/// Container registry push.
pub mod push;

/// Build state persistence.
pub mod saver;

/// Build status notifications.
pub mod notifier;

/// Build metrics collection and exposition.
pub mod metrics;

/// Clap CLI definitions.
pub mod cli;

// ── Mode entry points ────────────────────────────────────────────────

use std::collections::HashMap;

use secrecy::ExposeSecret;

use crate::builder::SelectedBuilder;
use crate::builder::{BuildContext, ImageBuilder};
use crate::cli::commands::build::BuildArgs;
use crate::cli::commands::daemon::DaemonArgs;
use crate::config::types::Config;
use crate::error::Error;
use crate::notifier::Notifier;
use crate::push::RegistryClient;
use crate::push::SelectedRegistry;
use crate::recipe::host_vars;
use crate::saver::Database;

/// Shared, immutable service references used throughout a daemon cycle.
///
/// Bundles all long-lived dependencies so that daemon helper functions
/// stay within the 5-parameter limit prescribed by CLAUDE.md.
struct DaemonServices<'a> {
    config: &'a Config,
    db: &'a saver::SelectedDatabase,
    builder: &'a SelectedBuilder,
    vcs_client: &'a scrapper::SelectedVcs,
    registry: &'a SelectedRegistry,
    notifier: Option<&'a notifier::SelectedNotifier>,
}

/// Execute a one-shot build (CLI mode).
///
/// Pipeline:
/// 1. Verify system requirements (docker, buildx, git)
/// 2. Load and resolve the recipe (flavors, host vars, profiles)
/// 3. Set up per-platform buildx builders
/// 4. Generate Dockerfile and run `docker buildx build`
/// 5. Optionally push the image to the registry
/// 6. Tear down builders
///
/// # Arguments
///
/// * `config` - Loaded configuration
/// * `args` - Build subcommand arguments
///
/// # Errors
///
/// Returns [`Error`] on recipe, build, or push failure.
pub async fn run_build(config: Config, args: BuildArgs) -> Result<(), Error> {
    // 1. System check (routed through configured Docker socket)
    tracing::info!(
        docker_host = config.docker.socket_uri,
        "verifying system requirements"
    );
    checker::verify_requirements(&config.docker.socket_uri).await?;

    // 2. Load recipe
    tracing::info!(recipe = %args.recipe.display(), "loading recipe");
    let raw_recipe = recipe::load(&args.recipe)?;

    // 3. Resolve flavors and variables
    let cli_overrides = args.flavor_overrides();
    let cli_overrides_ref = if cli_overrides.is_empty() {
        None
    } else {
        Some(&cli_overrides)
    };
    let recipe_stem = args
        .recipe
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let config_overrides = config.flavours.get(&recipe_stem);

    // Collect host variables
    // We need selected_flavours first to derive BUILD_TAGS_COMMA_SEP,
    // but resolve() needs host_vars. Two-step: pre-resolve flavours
    // to get build_tags, then collect host vars, then full resolve.
    let pre_flavours = recipe::resolve_flavours(&raw_recipe, config_overrides, cli_overrides_ref)?;
    let hvars = host_vars::collect(&args.tag, &pre_flavours);

    let resolved = recipe::resolve(raw_recipe, config_overrides, cli_overrides_ref, &hvars)?;

    tracing::info!(
        name = resolved.recipe.header.name,
        tag = args.tag,
        "recipe resolved"
    );

    // 4. Set up builder (CLI mode: persist = false)
    let builder: SelectedBuilder = builder::buildkit::BuildKitBuilder::new(
        config.docker.socket_uri.clone(),
        config.docker.builder_prefix.clone(),
        false, // CLI mode: destroy builders after build
    );

    // Run setup + build, guaranteeing cleanup on ALL exit paths
    let build_result = async {
        tracing::info!("setting up buildx builders");
        builder.setup_builders().await?;

        let platforms = args.platforms();
        let context = BuildContext::new(resolved, args.tag.clone(), platforms);
        builder.build(&context).await
    }
    .await;

    // Cleanup runs even if setup or build failed
    if let Err(e) = builder.cleanup().await {
        tracing::error!(error = %e, "builder cleanup failed");
    }

    let output = build_result?;

    tracing::info!(
        image = output.image_tag,
        duration_secs = output.duration.as_secs(),
        "build succeeded"
    );

    // 7. Push if requested
    if args.push {
        let registry: SelectedRegistry = push::oci::OciRegistry::new(
            config.docker.socket_uri.clone(),
            config.registry.url.clone(),
        );

        tracing::info!("authenticating with registry");
        registry.authenticate().await?;

        // Split image:tag for the push API
        let (image_name, image_tag) = output
            .image_tag
            .rsplit_once(':')
            .unwrap_or((&output.image_tag, "latest"));

        tracing::info!(image = output.image_tag, "pushing image");
        registry.push_image(image_name, image_tag).await?;

        tracing::info!("push completed");
    }

    Ok(())
}

/// Start the daemon (continuous polling mode).
///
/// 1. Verify system requirements
/// 2. Open database for build state persistence
/// 3. Set up persistent buildx builders
/// 4. Optionally initialize notifier
/// 5. Poll loop: for each recipe, fetch releases, skip already-built
///    or failed tags, build, push, save result, notify
/// 6. Sleep `poll_interval_secs` between cycles
/// 7. On SIGINT/SIGTERM: cleanup builders and exit
///
/// In daemon mode, buildx builders persist across polling cycles.
///
/// # Arguments
///
/// * `config` - Loaded configuration
/// * `args` - Daemon subcommand arguments (includes `--rpc` flag)
///
/// # Errors
///
/// Returns [`Error`] on fatal startup failure.  Individual build
/// failures are logged and persisted but do not stop the daemon.
pub async fn run_daemon(config: Config, args: DaemonArgs) -> Result<(), Error> {
    // 0. Enforce single instance
    let _lock = checker::ensure_singleton()?;

    // 1. System check
    checker::verify_requirements(&config.docker.socket_uri).await?;

    // 2. Open database
    let db: saver::SelectedDatabase = saver::redb::RedbDatabase::open(&config.database.path)?;
    tracing::info!(path = %config.database.path.display(), "database opened");

    // 3. Persistent builders (survive across polling cycles)
    let builder: SelectedBuilder = builder::buildkit::BuildKitBuilder::new(
        config.docker.socket_uri.clone(),
        config.docker.builder_prefix.clone(),
        true, // daemon mode: persist builders
    );
    builder.setup_builders().await?;
    tracing::info!("buildx builders ready (persistent)");

    // 4. Initialize notifier (optional)
    let notifier = init_notifier(&config);

    // 5. Initialize VCS client
    let secrets = config::load_secrets();
    let vcs_client: scrapper::SelectedVcs = scrapper::github::GithubClient::new(
        secrets.gh_user.as_ref().map(|s| s.expose_secret()),
        secrets.gh_pat.as_ref().map(|s| s.expose_secret()),
    )?;

    // 6. Registry client
    let registry: SelectedRegistry = push::oci::OciRegistry::new(
        config.docker.socket_uri.clone(),
        config.registry.url.clone(),
    );

    // Resolve daemon settings
    let daemon_cfg = config.daemon.as_ref();
    let poll_interval = daemon_cfg.map(|d| d.poll_interval_secs).unwrap_or(60);
    let max_builds = daemon_cfg.map(|d| d.max_builds_per_cycle).unwrap_or(1);

    // Filter recipes if --recipes was specified
    let recipe_filter: Vec<String> = args.recipes;

    tracing::info!(
        poll_interval_secs = poll_interval,
        max_builds_per_cycle = max_builds,
        rpc = args.rpc,
        "daemon starting"
    );

    // 7. Poll loop with graceful shutdown (SIGINT + SIGTERM)
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| {
            Error::Config(crate::error::ConfigError::Invalid(format!(
                "failed to register SIGTERM handler: {e}"
            )))
        })?;

    let services = DaemonServices {
        config: &config,
        db: &db,
        builder: &builder,
        vcs_client: &vcs_client,
        registry: &registry,
        notifier: notifier.as_ref(),
    };

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("SIGINT received, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received, shutting down");
                break;
            }
            _ = daemon_cycle(
                &services,
                max_builds,
                &recipe_filter,
            ) => {}
        }

        // Sleep between cycles
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("SIGINT received, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received, shutting down");
                break;
            }
            _ = tokio::time::sleep(
                std::time::Duration::from_secs(poll_interval)
            ) => {}
        }
    }

    // Cleanup: in daemon mode persist=true, so cleanup is a no-op
    // unless we explicitly want to tear down on exit
    tracing::info!("daemon stopped");
    Ok(())
}

/// Initialize the Telegram notifier if enabled and credentials are
/// available.
fn init_notifier(config: &Config) -> Option<notifier::SelectedNotifier> {
    if !config.notifier.enabled {
        return None;
    }

    let token = std::env::var("TELEGRAM_TOKEN").ok()?;
    let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok()?;

    match notifier::telegram::TelegramNotifier::new(&token, &chat_id) {
        Ok(n) => {
            tracing::info!("telegram notifier enabled");
            Some(n)
        },
        Err(e) => {
            tracing::error!(error = %e, "failed to init notifier");
            None
        },
    }
}

/// Execute one polling cycle across all recipes.
///
/// The `max_builds` budget is shared across all recipes in a single
/// cycle, preventing one cycle from exceeding the configured limit.
///
/// # Arguments
///
/// * `services` - Shared daemon service references
/// * `max_builds` - Maximum builds allowed in this cycle
/// * `recipe_filter` - If non-empty, only build these recipe stems
async fn daemon_cycle(services: &DaemonServices<'_>, max_builds: u32, recipe_filter: &[String]) {
    // Load all recipes
    let recipes = match recipe::load_all(&services.config.recipes_dir) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "failed to load recipes");
            return;
        },
    };

    let mut remaining_budget = max_builds;

    for (recipe_stem, raw_recipe) in &recipes {
        if remaining_budget == 0 {
            tracing::debug!("build budget exhausted for this cycle");
            break;
        }

        // Apply recipe filter if specified
        if !recipe_filter.is_empty() && !recipe_filter.contains(recipe_stem) {
            continue;
        }

        if let Err(e) =
            process_recipe(services, &mut remaining_budget, recipe_stem, raw_recipe).await
        {
            tracing::error!(
                recipe = recipe_stem,
                error = %e,
                "recipe processing failed"
            );
        }
    }
}

/// Process a single recipe: fetch releases, filter, build new tags.
///
/// `remaining_budget` is decremented after each build. When it reaches
/// zero, no more tags are built (across all recipes in the cycle).
///
/// # Arguments
///
/// * `services` - Shared daemon service references
/// * `remaining_budget` - Mutable counter of remaining builds allowed
/// * `recipe_stem` - Recipe file stem (e.g. `"cosmos-gaiad"`)
/// * `raw_recipe` - Parsed recipe definition
///
/// # Errors
///
/// Returns [`Error`] on VCS fetch failures.
async fn process_recipe(
    services: &DaemonServices<'_>,
    remaining_budget: &mut u32,
    recipe_stem: &str,
    raw_recipe: &recipe::types::Recipe,
) -> Result<(), Error> {
    use crate::scrapper::{TagFilter, VersionControlSystem};

    // Fetch releases from VCS
    let filter = TagFilter {
        include_patterns: raw_recipe.header.include_patterns.clone(),
        exclude_patterns: raw_recipe.header.exclude_patterns.clone(),
    };

    let releases = services
        .vcs_client
        .fetch_releases(&raw_recipe.header.repo, &filter)
        .await?;

    tracing::debug!(
        recipe = recipe_stem,
        total_releases = releases.len(),
        "fetched releases"
    );

    // Filter out already-built or failed tags
    let mut to_build = Vec::new();
    for release in &releases {
        if to_build.len() >= *remaining_budget as usize {
            break;
        }

        match services.db.is_built(recipe_stem, &release.tag).await {
            Ok(true) => {
                tracing::trace!(
                    recipe = recipe_stem,
                    tag = release.tag,
                    "skipping (already built)"
                );
                continue;
            },
            Ok(false) => {},
            Err(e) => {
                tracing::warn!(
                    recipe = recipe_stem,
                    tag = release.tag,
                    error = %e,
                    "db check failed, skipping tag"
                );
                continue;
            },
        }

        to_build.push(&release.tag);
    }

    if to_build.is_empty() {
        tracing::debug!(recipe = recipe_stem, "no new tags to build");
        return Ok(());
    }

    tracing::info!(
        recipe = recipe_stem,
        tags = ?to_build,
        "building new tags"
    );

    // Build each tag
    let config_overrides = services.config.flavours.get(recipe_stem);

    for tag in to_build {
        build_tag(services, config_overrides, recipe_stem, raw_recipe, tag).await;
        *remaining_budget = remaining_budget.saturating_sub(1);
        if *remaining_budget == 0 {
            break;
        }
    }

    Ok(())
}

/// Build a single tag, persisting the result and notifying.
///
/// Errors are caught and saved to the database -- they do not
/// propagate to the caller (daemon continues).
///
/// # Arguments
///
/// * `services` - Shared daemon service references
/// * `config_overrides` - Per-recipe flavor overrides from config
/// * `recipe_stem` - Recipe file stem (e.g. `"cosmos-gaiad"`)
/// * `raw_recipe` - Parsed recipe definition
/// * `tag` - Git tag to build
async fn build_tag(
    services: &DaemonServices<'_>,
    config_overrides: Option<&config::types::RecipeFlavourOverride>,
    recipe_stem: &str,
    raw_recipe: &recipe::types::Recipe,
    tag: &str,
) {
    let started_at = recipe::host_vars::utc_now();

    tracing::info!(recipe = recipe_stem, tag, "build starting");

    // Notify start
    if let Some(n) = services.notifier
        && let Err(e) = n.notify_build_start(recipe_stem, tag).await
    {
        tracing::warn!(error = %e, "start notification failed");
    }

    // Save in-progress record
    let mut record = saver::BuildRecord {
        recipe_name: recipe_stem.to_owned(),
        tag: tag.to_owned(),
        status: saver::BuildStatus::InProgress,
        image_tag: None,
        started_at: started_at.clone(),
        completed_at: None,
        duration_secs: None,
        error: None,
        flavours: HashMap::new(),
    };
    if let Err(e) = services.db.save_build(&record).await {
        tracing::error!(
            recipe = recipe_stem,
            tag,
            error = %e,
            "failed to save in-progress build record, skipping tag"
        );
        return;
    }

    // Resolve recipe
    let pre_flavours = match recipe::resolve_flavours(raw_recipe, config_overrides, None) {
        Ok(f) => f,
        Err(e) => {
            finish_build_failed(services.db, services.notifier, &mut record, &e.to_string()).await;
            return;
        },
    };

    let hvars = host_vars::collect(tag, &pre_flavours);

    let resolved = match recipe::resolve(raw_recipe.clone(), config_overrides, None, &hvars) {
        Ok(r) => r,
        Err(e) => {
            finish_build_failed(services.db, services.notifier, &mut record, &e.to_string()).await;
            return;
        },
    };

    // Build
    let platforms = vec!["linux/amd64".to_owned(), "linux/arm64".to_owned()];
    let context = BuildContext::new(resolved, tag.to_owned(), platforms);

    match services.builder.build(&context).await {
        Ok(output) => {
            // Push
            if let Err(e) = services.registry.authenticate().await {
                finish_build_failed(
                    services.db,
                    services.notifier,
                    &mut record,
                    &format!("push auth: {e}"),
                )
                .await;
                return;
            }
            let (img, img_tag) = output
                .image_tag
                .rsplit_once(':')
                .unwrap_or((&output.image_tag, "latest"));
            if let Err(e) = services.registry.push_image(img, img_tag).await {
                finish_build_failed(
                    services.db,
                    services.notifier,
                    &mut record,
                    &format!("push: {e}"),
                )
                .await;
                return;
            }

            // Success
            record.status = saver::BuildStatus::Success;
            record.image_tag = Some(output.image_tag.clone());
            record.completed_at = Some(recipe::host_vars::utc_now());
            record.duration_secs = Some(output.duration.as_secs());
            if let Err(e) = services.db.save_build(&record).await {
                tracing::error!(
                    recipe = recipe_stem,
                    tag,
                    error = %e,
                    "failed to save successful build record"
                );
            }

            tracing::info!(
                recipe = recipe_stem,
                tag,
                image = output.image_tag,
                duration_secs = output.duration.as_secs(),
                "build succeeded"
            );

            if let Some(n) = services.notifier
                && let Err(e) = n
                    .notify_build_success(recipe_stem, tag, output.duration)
                    .await
            {
                tracing::warn!(error = %e, "success notification failed");
            }
        },
        Err(e) => {
            finish_build_failed(services.db, services.notifier, &mut record, &e.to_string()).await;
        },
    }
}

/// Mark a build as failed: update the record, save to DB, notify.
async fn finish_build_failed(
    db: &saver::SelectedDatabase,
    notifier: Option<&notifier::SelectedNotifier>,
    record: &mut saver::BuildRecord,
    error: &str,
) {
    record.status = saver::BuildStatus::Failed;
    record.completed_at = Some(recipe::host_vars::utc_now());
    record.error = Some(error.to_owned());
    if let Err(e) = db.save_build(record).await {
        tracing::error!(
            recipe = record.recipe_name,
            tag = record.tag,
            error = %e,
            "failed to save failed build record"
        );
    }

    tracing::error!(
        recipe = record.recipe_name,
        tag = record.tag,
        error,
        "build failed"
    );

    if let Some(n) = notifier
        && let Err(e) = n
            .notify_build_failure(&record.recipe_name, &record.tag, error)
            .await
    {
        tracing::warn!(error = %e, "failure notification failed");
    }
}
