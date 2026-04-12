//! Go-specific build helpers.
//!
//! Generates `go build` commands with appropriate `-ldflags`, `-tags`,
//! and environment variables from a resolved recipe.  All output is
//! driven entirely by recipe data -- no chain-specific logic.

use std::collections::HashMap;

use crate::builder::template::TemplateEngine;
use crate::recipe::types::ResolvedRecipe;

// ── Dockerfile build script ──────────────────────────────────────────

/// Generate a shell script for the Dockerfile `RUN` instruction that
/// resolves build-time variables and executes `go build`.
///
/// The script:
/// 1. Assigns each `[variables]` shell command to a shell variable
/// 2. Constructs the full `go build` invocation
///
/// Host-time variables (already in `resolved_variables`) are expanded
/// inline.  Build-time variables (`{{lowercase}}` from `[variables]`)
/// become `$var_name` for shell interpolation.
///
/// # Arguments
///
/// * `recipe` - Fully resolved recipe
///
/// # Returns
///
/// A shell script body suitable for `RUN set -e; ... `.
pub fn generate_build_script(recipe: &ResolvedRecipe) -> String {
    let vars = &recipe.resolved_variables;

    let mut parts: Vec<String> = vec!["set -e".to_owned()];

    // Shell variable assignments from [variables]
    for (name, def) in &recipe.recipe.variables {
        parts.push(format!("{name}=$({cmd})", cmd = def.shell));
    }

    // Go build command
    let go_cmd = build_go_command(recipe, vars);
    parts.push(go_cmd);

    parts.join("; \\\n    ")
}

/// Construct the full `go build` command string with shell-interpolated
/// variables.
fn build_go_command(recipe: &ResolvedRecipe, vars: &HashMap<String, String>) -> String {
    let binary_type = recipe
        .selected_flavours
        .get_single("binary_type")
        .unwrap_or("dynamic");

    // Base linker flags for the selected binary type
    let base_flags = recipe
        .recipe
        .build
        .linker
        .flags
        .get(binary_type)
        .cloned()
        .unwrap_or_default();

    // -X flags from linker variables
    // First expand host-time vars, then convert remaining {{var}} to $var
    let x_flags: Vec<String> = recipe
        .recipe
        .build
        .linker
        .variables
        .iter()
        .map(|(path, val_template)| {
            let partially_expanded = TemplateEngine::render(val_template, vars);
            let shell_ready = template_to_shell(&partially_expanded);
            format!("-X '{path}={shell_ready}'")
        })
        .collect();

    let ldflags = if x_flags.is_empty() {
        base_flags
    } else {
        format!("{base_flags} {}", x_flags.join(" "))
    };

    // Build tags
    let tags = build_tags(recipe);

    // Build path
    let build_path = TemplateEngine::render(&recipe.recipe.build.path.path, vars);

    // Output binary
    let binary_name = &recipe.recipe.header.binary_name;

    // Assemble
    let mut cmd = String::from("go build -mod=readonly");
    if !tags.is_empty() {
        cmd.push_str(&format!(" \\\n      -tags={tags}"));
    }
    if !ldflags.is_empty() {
        cmd.push_str(&format!(" \\\n      -ldflags=\"{ldflags}\""));
    }
    cmd.push_str(&format!(" \\\n      -o /go/bin/{binary_name}"));
    cmd.push_str(&format!(" \\\n      {build_path}"));

    cmd
}

// ── Helpers (also used by non-Dockerfile paths) ──────────────────────

/// Construct the `-ldflags` string for `go build`.
///
/// Combines linker flags (static/dynamic) with `-X` variable injections.
///
/// # Arguments
///
/// * `recipe` - Resolved recipe with selected flavours
/// * `variables` - Resolved template variables
///
/// # Returns
///
/// The complete `-ldflags` string ready for `go build`.
///
/// # Examples
///
/// ```no_run
/// # use std::collections::HashMap;
/// # use dockermint::recipe::types::ResolvedRecipe;
/// # fn example(recipe: &ResolvedRecipe, vars: &HashMap<String, String>) {
/// let flags = dockermint::builder::go::build_ldflags(recipe, vars);
/// // e.g. "-linkmode=external -w -s -X 'path.Version=v1.0'"
/// # }
/// ```
pub fn build_ldflags(recipe: &ResolvedRecipe, variables: &HashMap<String, String>) -> String {
    let binary_type = recipe
        .selected_flavours
        .get_single("binary_type")
        .unwrap_or("dynamic");

    let base_flags = recipe
        .recipe
        .build
        .linker
        .flags
        .get(binary_type)
        .cloned()
        .unwrap_or_default();

    let x_flags: String = recipe
        .recipe
        .build
        .linker
        .variables
        .iter()
        .map(|(path, val_template)| {
            let resolved = TemplateEngine::render(val_template, variables);
            format!(" -X '{path}={resolved}'")
        })
        .collect();

    format!("{base_flags}{x_flags}")
}

/// Construct the `-tags` argument for `go build`.
///
/// # Arguments
///
/// * `recipe` - Resolved recipe with selected flavours
///
/// # Returns
///
/// Comma-separated build tags string, or empty string if none.
pub fn build_tags(recipe: &ResolvedRecipe) -> String {
    let mut tags: Vec<&str> = Vec::new();

    if let Some(selected) = recipe.selected_flavours.get_multiple("build_tags") {
        tags.extend(selected.iter().map(String::as_str));
    }

    // Add db_backend as a build tag if it isn't the default goleveldb
    if let Some(db) = recipe.selected_flavours.get_single("db_backend")
        && db != "goleveldb"
    {
        tags.push(db);
    }

    tags.join(",")
}

/// Build the full `go build` command arguments.
///
/// # Arguments
///
/// * `recipe` - Resolved recipe
/// * `variables` - Resolved template variables
///
/// # Returns
///
/// A vector of command-line arguments for `go build`.
pub fn build_args(recipe: &ResolvedRecipe, variables: &HashMap<String, String>) -> Vec<String> {
    let mut args = vec!["build".to_owned(), "-mod=readonly".to_owned()];

    let tags = build_tags(recipe);
    if !tags.is_empty() {
        args.push(format!("-tags={tags}"));
    }

    let ldflags = build_ldflags(recipe, variables);
    if !ldflags.is_empty() {
        args.push(format!("-ldflags={ldflags}"));
    }

    // Output path
    args.push("-o".to_owned());
    args.push(format!("/go/bin/{}", recipe.recipe.header.binary_name));

    // Build target path
    let build_path =
        crate::builder::template::TemplateEngine::render(&recipe.recipe.build.path.path, variables);
    args.push(build_path);

    args
}

// ── Shell conversion ─────────────────────────────────────────────────

/// Convert remaining `{{var}}` template placeholders to `$var` for
/// shell interpolation inside a Dockerfile `RUN` instruction.
///
/// # Arguments
///
/// * `s` - String that may contain unresolved `{{placeholders}}`
///
/// # Returns
///
/// A new string where every `{{name}}` is replaced with `$name`.
///
/// # Examples
///
/// ```
/// # use dockermint::builder::go::template_to_shell;
/// assert_eq!(template_to_shell("v={{version}}"), "v=$version");
/// assert_eq!(template_to_shell("plain"), "plain");
/// ```
pub fn template_to_shell(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            i += 2; // skip {{
            let start = i;
            while i + 1 < len && !(bytes[i] == b'}' && bytes[i + 1] == b'}') {
                i += 1;
            }
            result.push('$');
            result.push_str(&s[start..i]);
            if i + 1 < len {
                i += 2; // skip }}
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe;
    use std::path::Path;

    #[test]
    fn template_to_shell_converts() {
        assert_eq!(template_to_shell("{{repo_version}}"), "$repo_version");
        assert_eq!(template_to_shell("v={{ver}}-{{tag}}"), "v=$ver-$tag");
    }

    #[test]
    fn template_to_shell_no_placeholders() {
        assert_eq!(template_to_shell("plain text"), "plain text");
    }

    #[test]
    fn template_to_shell_already_resolved() {
        assert_eq!(template_to_shell("gaiad"), "gaiad");
    }

    #[test]
    fn build_tags_returns_empty_for_no_tags() {
        let recipe = make_minimal_recipe();
        assert!(build_tags(&recipe).is_empty());
    }

    #[test]
    fn generate_build_script_cosmos() {
        let path = Path::new("recipes/cosmos-gaiad.toml");
        if !path.exists() {
            return;
        }

        let raw = recipe::load(path).expect("parse");
        let mut host_vars = HashMap::new();
        host_vars.insert("HOST_ARCH".to_owned(), "x86_64".to_owned());
        host_vars.insert("BUILD_TAGS_COMMA_SEP".to_owned(), "netgo,muslc".to_owned());
        host_vars.insert("repository_path".to_owned(), "/workspace".to_owned());

        let resolved = recipe::resolve(raw, None, None, &host_vars).expect("resolve");
        let script = generate_build_script(&resolved);

        assert!(script.starts_with("set -e"), "starts with set -e");
        assert!(script.contains("go build"), "contains go build");
        assert!(script.contains("-tags=netgo,muslc"), "has build tags");
        assert!(script.contains("-ldflags="), "has ldflags");
        assert!(script.contains("/go/bin/gaiad"), "output binary");
        assert!(script.contains("/workspace/cmd/gaiad"), "build path");
        // Build-time vars should be shell interpolated
        assert!(
            script.contains("$repo_version"),
            "shell var for repo_version"
        );
        assert!(script.contains("$repo_commit"), "shell var for repo_commit");
    }

    fn make_minimal_recipe() -> ResolvedRecipe {
        use crate::recipe::types::*;
        ResolvedRecipe {
            recipe: Recipe {
                meta: RecipeMeta {
                    schema_version: 1,
                    min_dockermint_version: "0.1.0".to_owned(),
                },
                header: RecipeHeader {
                    name: "Test".to_owned(),
                    repo: String::new(),
                    build_type: "golang".to_owned(),
                    binary_name: "testd".to_owned(),
                    include_patterns: String::new(),
                    exclude_patterns: String::new(),
                },
                flavours: RecipeFlavours {
                    available: HashMap::new(),
                    default: HashMap::new(),
                },
                scrapper: RecipeScrapper {
                    image: String::new(),
                    install: String::new(),
                    env: Vec::new(),
                    method: String::new(),
                    directory: String::new(),
                },
                variables: HashMap::new(),
                builder: Default::default(),
                pre_build: Vec::new(),
                build: RecipeBuild {
                    env: HashMap::new(),
                    linker: LinkerConfig {
                        flags: HashMap::new(),
                        variables: HashMap::new(),
                    },
                    path: BuildPath {
                        path: String::new(),
                    },
                },
                user: HashMap::new(),
                copy: RecipeCopySection(toml::Table::new()),
                expose: RecipeExpose { ports: Vec::new() },
                labels: HashMap::new(),
                image: RecipeImage { tag: String::new() },
                profiles: HashMap::new(),
            },
            selected_flavours: Default::default(),
            resolved_variables: HashMap::new(),
        }
    }
}
