//! Generic Dockerfile generation from a resolved recipe.
//!
//! The generator is **fully data-driven**: every instruction comes from
//! recipe fields.  Adding a new chain only requires a new TOML recipe
//! file -- zero Rust code changes.
//!
//! Build-type dispatch (e.g. `"golang"`) routes to the appropriate
//! sub-module for the build command fragment.  Adding a truly new build
//! system (not a new chain) requires one match arm here and one module.

use std::collections::HashMap;

use crate::builder::template::TemplateEngine;
use crate::error::BuilderError;
use crate::recipe::types::ResolvedRecipe;

/// Generate a complete multi-stage Dockerfile from a resolved recipe.
///
/// # Arguments
///
/// * `recipe` - A fully resolved recipe (flavors, host vars, profiles)
///
/// # Returns
///
/// The Dockerfile content as a [`String`].
///
/// # Errors
///
/// - [`BuilderError::DockerfileGeneration`] for unsupported build types
///   or missing required data.
///
/// # Examples
///
/// ```no_run
/// # use dockermint::recipe::types::ResolvedRecipe;
/// # fn example(resolved: &ResolvedRecipe) -> Result<(), Box<dyn std::error::Error>> {
/// let dockerfile = dockermint::builder::dockerfile::generate(resolved)?;
/// std::fs::write("Dockerfile", &dockerfile)?;
/// # Ok(())
/// # }
/// ```
pub fn generate(recipe: &ResolvedRecipe) -> Result<String, BuilderError> {
    let vars = &recipe.resolved_variables;
    let render = |s: &str| TemplateEngine::render(s, vars);

    let mut df = DockerfileWriter::new();

    // ================================================================
    // Stage 1: Build
    // ================================================================
    df.separator();
    df.comment("Stage 1: Build");
    df.separator();

    let scrapper_image = render(&recipe.recipe.scrapper.image);
    df.emit_from(&scrapper_image, "builder");
    df.blank();

    // -- Install dependencies ----------------------------------------
    // Merge scrapper install + builder install (distro auto-detected
    // from scrapper image name against builder.install keys).
    let mut install_parts: Vec<String> = Vec::new();
    if !recipe.recipe.scrapper.install.is_empty() {
        install_parts.push(render(&recipe.recipe.scrapper.install));
    }
    if let Some(cmd) = detect_install_command(&scrapper_image, &recipe.recipe.builder.install) {
        install_parts.push(render(cmd));
    }
    if !install_parts.is_empty() {
        df.run(&install_parts.join(" && "));
    }
    df.blank();

    // -- Build arguments -----------------------------------------------
    // Secrets (GH_USER, GH_PAT) are mounted via --mount=type=secret,
    // NOT passed as ARGs (which leak into layer history).
    df.arg("GIT_TAG", None);
    df.blank();

    // -- Clone repository --------------------------------------------
    generate_clone_commands(
        &mut df,
        &recipe.recipe.header.repo,
        &recipe.recipe.scrapper.method,
    );
    df.blank();

    // -- Checkout tag and set workdir --------------------------------
    let workdir = render(&recipe.recipe.scrapper.directory);
    df.workdir(&workdir);
    df.run("git checkout ${GIT_TAG}");
    df.blank();

    // -- Build environment variables ---------------------------------
    for (k, v) in &recipe.recipe.build.env {
        df.env_var(k, &render(v));
    }
    if !recipe.recipe.build.env.is_empty() {
        df.blank();
    }

    // -- Pre-build steps (conditional on active flavors) -------------
    let mut had_pre_build = false;
    for step in &recipe.recipe.pre_build {
        if recipe.selected_flavours.has_value(&step.condition) {
            let src = render(&step.source);
            let dst = render(&step.dest);
            match step.instruction.to_uppercase().as_str() {
                "ADD" => df.add(&src, &dst),
                "RUN" => df.run(&src),
                "COPY" => df.copy_from("builder", &src, &dst),
                inst => df.raw(&format!("{inst} {src} {dst}")),
            }
            had_pre_build = true;
        }
    }
    if had_pre_build {
        df.blank();
    }

    // -- Build command (dispatched by build type) --------------------
    let build_script = generate_build_command(recipe)?;
    df.run(&build_script);
    df.blank();

    // ================================================================
    // Stage 2: Runtime
    // ================================================================
    df.separator();
    df.comment("Stage 2: Runtime");
    df.separator();

    let running_env = recipe
        .selected_flavours
        .get_single("running_env")
        .unwrap_or("alpine3.23");
    let runner_image = running_env_to_image(running_env);
    df.emit_from(&runner_image, "runner");
    df.blank();

    // -- User creation -----------------------------------------------
    let running_user = recipe
        .selected_flavours
        .get_single("running_user")
        .unwrap_or("root");
    if running_user != "root"
        && let Some(user_cfg) = recipe.recipe.user.get(running_user)
        && let Some(cmd) = user_creation_command(running_env, user_cfg)
    {
        df.run(&cmd);
        df.blank();
    }

    // -- Copy artifacts from builder ---------------------------------
    let mut entrypoint_dest: Option<String> = None;
    for (src, entry) in recipe.recipe.copy.always_entries() {
        let rendered_dest = render(&entry.dest);
        df.copy_from("builder", &render(src), &rendered_dest);
        if entry.copy_type == "entrypoint" {
            entrypoint_dest = Some(rendered_dest);
        }
    }

    // Conditional copies (keyed by binary_type flavor)
    let binary_type = recipe
        .selected_flavours
        .get_single("binary_type")
        .unwrap_or("dynamic");
    for (src, entry) in recipe.recipe.copy.conditional_entries(binary_type) {
        df.copy_from("builder", &render(&src), &render(&entry.dest));
    }
    df.blank();

    // -- Expose ports ------------------------------------------------
    let ports: Vec<u16> = recipe.recipe.expose.ports.iter().map(|p| p.port).collect();
    if !ports.is_empty() {
        df.expose(&ports);
        df.blank();
    }

    // -- OCI Labels --------------------------------------------------
    if !recipe.recipe.labels.is_empty() {
        for (k, v) in &recipe.recipe.labels {
            df.label(k, &render(v));
        }
        df.blank();
    }

    // -- Non-root user -----------------------------------------------
    if running_user != "root"
        && let Some(user_cfg) = recipe.recipe.user.get(running_user)
    {
        df.user(&user_cfg.username);
        df.blank();
    }

    // -- Entrypoint --------------------------------------------------
    if let Some(ep) = &entrypoint_dest {
        df.entrypoint(&[ep.as_str()]);
    }

    Ok(df.build())
}

// ── build-type dispatch ──────────────────────────────────────────────

/// Dispatch to the build-type-specific script generator.
///
/// Adding a new build system (e.g. Rust, Node) only requires one match
/// arm here and one module.
fn generate_build_command(recipe: &ResolvedRecipe) -> Result<String, BuilderError> {
    match recipe.recipe.header.build_type.as_str() {
        "golang" => Ok(crate::builder::go::generate_build_script(recipe)),
        other => Err(BuilderError::DockerfileGeneration(format!(
            "unsupported build type: {other}"
        ))),
    }
}

// ── clone commands ───────────────────────────────────────────────────

/// Emit git clone instructions based on the scrapper method.
///
/// Uses BuildKit `--mount=type=secret` to avoid leaking credentials
/// into Docker image layer history.
fn generate_clone_commands(df: &mut DockerfileWriter, repo_url: &str, method: &str) {
    let host = strip_protocol(repo_url);
    match method {
        "try-authenticated-clone" => {
            df.comment("Clone: try authenticated via BuildKit secrets, fall back to public");
            df.comment("Secrets mounted at runtime -- never baked into image layers");
            df.raw(&format!(
                "RUN --mount=type=secret,id=gh_user \\\n\
                 \x20   --mount=type=secret,id=gh_pat \\\n\
                 \x20   GH_USER=$(cat /run/secrets/gh_user 2>/dev/null) && \\\n\
                 \x20   GH_PAT=$(cat /run/secrets/gh_pat 2>/dev/null) && \\\n\
                 \x20   git clone \"https://${{GH_USER}}:${{GH_PAT}}@{host}\" /workspace 2>/dev/null || \\\n\
                 \x20   git clone {repo_url} /workspace"
            ));
        },
        _ => {
            df.run(&format!("git clone {repo_url} /workspace"));
        },
    }
}

/// Strip `https://` or `http://` prefix.
fn strip_protocol(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

// ── image helpers ────────────────────────────────────────────────────

/// Map a `running_env` flavor value to a Docker image reference.
///
/// Convention: split at the first digit boundary.
///   `"alpine3.23"` -> `"alpine:3.23"`
///
/// Full image references (containing `/` or `:`) pass through.
fn running_env_to_image(running_env: &str) -> String {
    // Pass through if already a full reference
    if running_env.contains('/') || running_env.contains(':') {
        return running_env.to_owned();
    }

    // Split at first digit: "alpine3.23" -> ("alpine", "3.23")
    if let Some(pos) = running_env.find(|c: char| c.is_ascii_digit()) {
        let (name, version) = running_env.split_at(pos);
        return format!("{name}:{version}");
    }

    // Non-versioned well-known environments
    match running_env {
        "bookworm" => "debian:bookworm-slim".to_owned(),
        "distroless" => "gcr.io/distroless/static-debian12".to_owned(),
        other => other.to_owned(),
    }
}

/// Auto-detect which `builder.install` command to use based on the
/// scrapper image name.
///
/// Matches the **longest** key that appears as a substring of `image`.
/// This is fully data-driven: adding `fedora = "dnf install ..."`
/// to a recipe's `[builder.install]` works automatically if the
/// scrapper image contains `"fedora"`.
fn detect_install_command<'a>(
    image: &str,
    install_commands: &'a HashMap<String, String>,
) -> Option<&'a str> {
    install_commands
        .iter()
        .filter(|(distro, _)| image.contains(distro.as_str()))
        .max_by_key(|(distro, _)| distro.len())
        .map(|(_, cmd)| cmd.as_str())
}

/// Generate user creation command appropriate for the runner distro.
fn user_creation_command(
    running_env: &str,
    user_cfg: &crate::recipe::types::UserConfig,
) -> Option<String> {
    let u = &user_cfg.username;
    let uid = user_cfg.uid;
    let gid = user_cfg.gid;

    if running_env.contains("distroless") {
        // Distroless has no shell -- user set via numeric UID only
        return None;
    }

    if running_env.contains("alpine") {
        Some(format!(
            "addgroup -g {gid} {u} && adduser -u {uid} -G {u} -D {u}"
        ))
    } else {
        Some(format!(
            "groupadd -g {gid} {u} && useradd -u {uid} -g {gid} -M {u}"
        ))
    }
}

// ── DockerfileWriter ─────────────────────────────────────────────────

/// Line-oriented builder for Dockerfile content.
struct DockerfileWriter {
    lines: Vec<String>,
}

impl DockerfileWriter {
    fn new() -> Self {
        Self {
            lines: Vec::with_capacity(64),
        }
    }

    fn separator(&mut self) {
        self.lines
            .push("# ================================================================".to_owned());
    }

    fn comment(&mut self, text: &str) {
        self.lines.push(format!("# {text}"));
    }

    fn blank(&mut self) {
        self.lines.push(String::new());
    }

    fn emit_from(&mut self, image: &str, name: &str) {
        self.lines.push(format!("FROM {image} AS {name}"));
    }

    fn run(&mut self, cmd: &str) {
        self.lines.push(format!("RUN {cmd}"));
    }

    fn env_var(&mut self, key: &str, value: &str) {
        self.lines.push(format!("ENV {key}={value}"));
    }

    fn workdir(&mut self, path: &str) {
        self.lines.push(format!("WORKDIR {path}"));
    }

    fn arg(&mut self, name: &str, default: Option<&str>) {
        match default {
            Some(val) => self.lines.push(format!("ARG {name}={val}")),
            None => self.lines.push(format!("ARG {name}")),
        }
    }

    fn copy_from(&mut self, stage: &str, src: &str, dst: &str) {
        self.lines.push(format!("COPY --from={stage} {src} {dst}"));
    }

    fn add(&mut self, src: &str, dst: &str) {
        self.lines.push(format!("ADD {src} {dst}"));
    }

    fn expose(&mut self, ports: &[u16]) {
        let s: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
        self.lines.push(format!("EXPOSE {}", s.join(" ")));
    }

    fn label(&mut self, key: &str, value: &str) {
        self.lines.push(format!("LABEL {key}=\"{value}\""));
    }

    fn user(&mut self, username: &str) {
        self.lines.push(format!("USER {username}"));
    }

    fn entrypoint(&mut self, args: &[&str]) {
        let parts: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
        self.lines
            .push(format!("ENTRYPOINT [{}]", parts.join(", ")));
    }

    fn raw(&mut self, line: &str) {
        self.lines.push(line.to_owned());
    }

    fn build(self) -> String {
        self.lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_env_to_image_versioned() {
        assert_eq!(running_env_to_image("alpine3.23"), "alpine:3.23");
        assert_eq!(running_env_to_image("ubuntu24.04"), "ubuntu:24.04");
    }

    #[test]
    fn running_env_to_image_named() {
        assert_eq!(running_env_to_image("bookworm"), "debian:bookworm-slim");
        assert_eq!(
            running_env_to_image("distroless"),
            "gcr.io/distroless/static-debian12"
        );
    }

    #[test]
    fn running_env_to_image_passthrough() {
        assert_eq!(
            running_env_to_image("ghcr.io/foo/bar:latest"),
            "ghcr.io/foo/bar:latest"
        );
    }

    #[test]
    fn detect_install_command_matches_alpine() {
        let mut cmds = HashMap::new();
        cmds.insert("alpine".to_owned(), "apk add git".to_owned());
        cmds.insert("ubuntu".to_owned(), "apt-get install git".to_owned());

        assert_eq!(
            detect_install_command("golang:1.23-alpine3.21", &cmds),
            Some("apk add git")
        );
    }

    #[test]
    fn detect_install_command_no_match() {
        let cmds = HashMap::new();
        assert_eq!(detect_install_command("golang:1.23", &cmds), None);
    }

    #[test]
    fn strip_protocol_https() {
        assert_eq!(
            strip_protocol("https://github.com/cosmos/gaia"),
            "github.com/cosmos/gaia"
        );
    }

    #[test]
    fn generate_cosmos_dockerfile() {
        use crate::recipe;
        use std::path::Path;

        let path = Path::new("recipes/cosmos-gaiad.toml");
        if !path.exists() {
            return;
        }

        let raw = recipe::load(path).expect("parse");
        let mut host_vars = std::collections::HashMap::new();
        host_vars.insert("HOST_ARCH".to_owned(), "x86_64".to_owned());
        host_vars.insert("SEMVER_TAG".to_owned(), "v21.0.1".to_owned());
        host_vars.insert(
            "CREATION_TIMESTAMP".to_owned(),
            "2026-04-12T00:00:00Z".to_owned(),
        );
        host_vars.insert("BUILD_TAGS_COMMA_SEP".to_owned(), "netgo,muslc".to_owned());
        host_vars.insert("repository_path".to_owned(), "/workspace".to_owned());

        let resolved = recipe::resolve(raw, None, None, &host_vars).expect("resolve");
        let dockerfile = generate(&resolved).expect("generate");

        // Verify key structural elements
        assert!(dockerfile.contains("FROM golang:"), "has builder stage");
        assert!(dockerfile.contains("AS builder"), "builder alias");
        assert!(dockerfile.contains("AS runner"), "runner alias");
        assert!(dockerfile.contains("FROM alpine:3.23"), "runner image");
        assert!(dockerfile.contains("ENTRYPOINT"), "has entrypoint");
        assert!(dockerfile.contains("gaiad"), "references binary");
        assert!(dockerfile.contains("EXPOSE"), "has expose");
        assert!(dockerfile.contains("go build"), "has go build");
        assert!(dockerfile.contains("CGO_ENABLED"), "has build env");
    }

    #[test]
    fn generate_kyve_dockerfile() {
        use crate::recipe;
        use std::path::Path;

        let path = Path::new("recipes/kyve-kyved.toml");
        if !path.exists() {
            return;
        }

        let raw = recipe::load(path).expect("parse");
        let mut host_vars = std::collections::HashMap::new();
        host_vars.insert("HOST_ARCH".to_owned(), "x86_64".to_owned());
        host_vars.insert("SEMVER_TAG".to_owned(), "v1.5.0".to_owned());
        host_vars.insert(
            "CREATION_TIMESTAMP".to_owned(),
            "2026-04-12T00:00:00Z".to_owned(),
        );
        host_vars.insert("BUILD_TAGS_COMMA_SEP".to_owned(), "netgo,muslc".to_owned());
        host_vars.insert("repository_path".to_owned(), "/workspace".to_owned());

        let resolved = recipe::resolve(raw, None, None, &host_vars).expect("resolve");
        let dockerfile = generate(&resolved).expect("generate");

        assert!(dockerfile.contains("kyved"), "references kyved binary");
        // Kyve has network profiles -> denom should be in ldflags
        assert!(dockerfile.contains("ukyve"), "mainnet denom injected");
    }
}
