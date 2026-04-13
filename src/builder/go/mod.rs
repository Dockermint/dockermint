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

    // ── Test helpers ──────────────────────────────────────────────────

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

    /// Config for building test recipes.
    #[derive(Default)]
    struct TestRecipeConfig<'a> {
        binary_name: &'a str,
        binary_type: Option<&'a str>,
        db_backend: Option<&'a str>,
        build_tags: Option<Vec<&'a str>>,
        linker_flags: HashMap<String, String>,
        linker_variables: HashMap<String, String>,
        build_path: &'a str,
        variables: HashMap<String, crate::recipe::types::VariableDefinition>,
    }

    /// Build a recipe with specific flavours, linker config, and build path.
    fn make_recipe_with(cfg: TestRecipeConfig<'_>) -> ResolvedRecipe {
        let TestRecipeConfig {
            binary_name,
            binary_type,
            db_backend,
            build_tags,
            linker_flags,
            linker_variables,
            build_path,
            variables,
        } = cfg;
        use crate::recipe::types::*;

        let mut selections = HashMap::new();
        if let Some(bt) = binary_type {
            selections.insert("binary_type".to_owned(), FlavorValue::Single(bt.to_owned()));
        }
        if let Some(db) = db_backend {
            selections.insert("db_backend".to_owned(), FlavorValue::Single(db.to_owned()));
        }
        if let Some(tags) = build_tags {
            selections.insert(
                "build_tags".to_owned(),
                FlavorValue::Multiple(tags.into_iter().map(str::to_owned).collect()),
            );
        }

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
                    binary_name: binary_name.to_owned(),
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
                variables,
                builder: Default::default(),
                pre_build: Vec::new(),
                build: RecipeBuild {
                    env: HashMap::new(),
                    linker: LinkerConfig {
                        flags: linker_flags,
                        variables: linker_variables,
                    },
                    path: BuildPath {
                        path: build_path.to_owned(),
                    },
                },
                user: HashMap::new(),
                copy: RecipeCopySection(toml::Table::new()),
                expose: RecipeExpose { ports: Vec::new() },
                labels: HashMap::new(),
                image: RecipeImage { tag: String::new() },
                profiles: HashMap::new(),
            },
            selected_flavours: SelectedFlavours { selections },
            resolved_variables: HashMap::new(),
        }
    }

    // ── build_tags tests ──────────────────────────────────────────────

    #[test]
    fn build_tags_with_multiple_tags() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            build_tags: Some(vec!["netgo", "muslc"]),
            ..Default::default()
        });
        assert_eq!(build_tags(&recipe), "netgo,muslc");
    }

    #[test]
    fn build_tags_adds_non_default_db_backend() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            db_backend: Some("pebbledb"),
            ..Default::default()
        });
        assert_eq!(build_tags(&recipe), "pebbledb");
    }

    #[test]
    fn build_tags_skips_goleveldb() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            db_backend: Some("goleveldb"),
            ..Default::default()
        });
        assert!(
            build_tags(&recipe).is_empty(),
            "goleveldb must not appear in build tags"
        );
    }

    #[test]
    fn build_tags_combines_tags_and_db_backend() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            db_backend: Some("pebbledb"),
            build_tags: Some(vec!["netgo"]),
            ..Default::default()
        });
        let tags = build_tags(&recipe);
        assert!(tags.contains("netgo"), "must contain build_tags entry");
        assert!(tags.contains("pebbledb"), "must contain db_backend");
        assert_eq!(tags, "netgo,pebbledb");
    }

    // ── build_ldflags tests ───────────────────────────────────────────

    #[test]
    fn build_ldflags_empty_when_no_config() {
        let recipe = make_minimal_recipe();
        let vars = HashMap::new();
        assert!(
            build_ldflags(&recipe, &vars).is_empty(),
            "no flags or variables means empty ldflags"
        );
    }

    #[test]
    fn build_ldflags_uses_dynamic_flags_by_default() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());
        flags.insert("static".to_owned(), "-linkmode=external".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_flags: flags,
            ..Default::default()
        });
        let result = build_ldflags(&recipe, &HashMap::new());
        assert_eq!(result, "-w -s");
    }

    #[test]
    fn build_ldflags_uses_static_flags_when_selected() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());
        flags.insert("static".to_owned(), "-linkmode=external -w -s".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            binary_type: Some("static"),
            linker_flags: flags,
            ..Default::default()
        });
        let result = build_ldflags(&recipe, &HashMap::new());
        assert_eq!(result, "-linkmode=external -w -s");
    }

    #[test]
    fn build_ldflags_includes_x_flags() {
        let mut linker_vars = HashMap::new();
        linker_vars.insert("main.Version".to_owned(), "{{VERSION}}".to_owned());

        let mut vars = HashMap::new();
        vars.insert("VERSION".to_owned(), "v1.0.0".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_variables: linker_vars,
            ..Default::default()
        });
        let result = build_ldflags(&recipe, &vars);
        assert!(
            result.contains("-X 'main.Version=v1.0.0'"),
            "expected -X flag, got: {result}"
        );
    }

    #[test]
    fn build_ldflags_combines_base_and_x_flags() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());

        let mut linker_vars = HashMap::new();
        linker_vars.insert("main.AppName".to_owned(), "myapp".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_flags: flags,
            linker_variables: linker_vars,
            ..Default::default()
        });
        let result = build_ldflags(&recipe, &HashMap::new());
        assert!(result.starts_with("-w -s"), "starts with base flags");
        assert!(
            result.contains("-X 'main.AppName=myapp'"),
            "contains -X flag"
        );
    }

    // ── build_args tests ──────────────────────────────────────────────

    #[test]
    fn build_args_minimal() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "mybin",
            build_path: "./cmd/mybin",
            ..Default::default()
        });
        let args = build_args(&recipe, &HashMap::new());

        assert_eq!(args[0], "build");
        assert_eq!(args[1], "-mod=readonly");
        assert!(args.contains(&"-o".to_owned()), "must contain -o flag");
        assert!(
            args.contains(&"/go/bin/mybin".to_owned()),
            "must contain output binary path"
        );
        assert!(
            args.contains(&"./cmd/mybin".to_owned()),
            "must contain build target path"
        );
    }

    #[test]
    fn build_args_omits_tags_when_empty() {
        let recipe = make_minimal_recipe();
        let args = build_args(&recipe, &HashMap::new());
        assert!(
            !args.iter().any(|a| a.starts_with("-tags=")),
            "no -tags= when tags are empty"
        );
    }

    #[test]
    fn build_args_includes_tags_when_present() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            build_tags: Some(vec!["netgo", "muslc"]),
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let args = build_args(&recipe, &HashMap::new());
        assert!(
            args.contains(&"-tags=netgo,muslc".to_owned()),
            "must include -tags= with comma-separated tags"
        );
    }

    #[test]
    fn build_args_omits_ldflags_when_empty() {
        let recipe = make_minimal_recipe();
        let args = build_args(&recipe, &HashMap::new());
        assert!(
            !args.iter().any(|a| a.starts_with("-ldflags=")),
            "no -ldflags= when ldflags are empty"
        );
    }

    #[test]
    fn build_args_includes_ldflags_when_present() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_flags: flags,
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let args = build_args(&recipe, &HashMap::new());
        assert!(
            args.iter().any(|a| a.starts_with("-ldflags=")),
            "must include -ldflags= when flags present"
        );
        assert!(
            args.iter().any(|a| a.contains("-w -s")),
            "ldflags must contain the actual flag content"
        );
    }

    #[test]
    fn build_args_resolves_template_in_build_path() {
        let mut vars = HashMap::new();
        vars.insert("repo_path".to_owned(), "/workspace".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            build_path: "{{repo_path}}/cmd/testd",
            ..Default::default()
        });
        let args = build_args(&recipe, &vars);
        assert!(
            args.contains(&"/workspace/cmd/testd".to_owned()),
            "template in build path must be resolved"
        );
    }

    // ── generate_build_script tests ───────────────────────────────────

    #[test]
    fn generate_build_script_starts_with_set_e() {
        let recipe = make_minimal_recipe();
        let script = generate_build_script(&recipe);
        assert!(
            script.starts_with("set -e"),
            "script must begin with set -e"
        );
    }

    #[test]
    fn generate_build_script_contains_go_build() {
        let recipe = make_minimal_recipe();
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("go build -mod=readonly"),
            "script must contain go build -mod=readonly"
        );
    }

    #[test]
    fn generate_build_script_includes_variable_assignments() {
        use crate::recipe::types::VariableDefinition;

        let mut variables = HashMap::new();
        variables.insert(
            "repo_version".to_owned(),
            VariableDefinition {
                shell: "git describe --tags".to_owned(),
            },
        );

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            build_path: "./cmd/testd",
            variables,
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("repo_version=$(git describe --tags)"),
            "script must contain variable assignment, got: {script}"
        );
    }

    #[test]
    fn generate_build_script_uses_semicolon_backslash_join() {
        let recipe = make_minimal_recipe();
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("; \\\n    "),
            "parts must be joined with '; \\\\\\n    '"
        );
    }

    #[test]
    fn generate_build_script_includes_output_binary() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "mybin",
            build_path: "./cmd/mybin",
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("/go/bin/mybin"),
            "script must contain output binary path"
        );
    }

    #[test]
    fn generate_build_script_includes_tags_when_present() {
        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            build_tags: Some(vec!["netgo"]),
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("-tags=netgo"),
            "script must contain -tags= when tags present"
        );
    }

    #[test]
    fn generate_build_script_omits_tags_when_empty() {
        let recipe = make_minimal_recipe();
        let script = generate_build_script(&recipe);
        assert!(
            !script.contains("-tags="),
            "script must not contain -tags= when no tags"
        );
    }

    #[test]
    fn generate_build_script_includes_ldflags_when_present() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_flags: flags,
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("-ldflags=\"-w -s\""),
            "script must contain -ldflags with flags, got: {script}"
        );
    }

    #[test]
    fn generate_build_script_omits_ldflags_when_empty() {
        let recipe = make_minimal_recipe();
        let script = generate_build_script(&recipe);
        assert!(
            !script.contains("-ldflags="),
            "script must not contain -ldflags= when empty"
        );
    }

    #[test]
    fn generate_build_script_shell_interpolation_for_x_flags() {
        let mut linker_vars = HashMap::new();
        linker_vars.insert("main.Version".to_owned(), "{{repo_version}}".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_variables: linker_vars,
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(
            script.contains("$repo_version"),
            "unresolved template vars must become shell vars, got: {script}"
        );
        assert!(
            !script.contains("{{repo_version}}"),
            "template braces must be replaced"
        );
    }

    #[test]
    fn generate_build_script_x_flags_combined_with_base() {
        let mut flags = HashMap::new();
        flags.insert("dynamic".to_owned(), "-w -s".to_owned());

        let mut linker_vars = HashMap::new();
        linker_vars.insert("main.Version".to_owned(), "hardcoded_val".to_owned());

        let recipe = make_recipe_with(TestRecipeConfig {
            binary_name: "testd",
            linker_flags: flags,
            linker_variables: linker_vars,
            build_path: "./cmd/testd",
            ..Default::default()
        });
        let script = generate_build_script(&recipe);
        assert!(script.contains("-w -s"), "must include base linker flags");
        assert!(
            script.contains("-X 'main.Version=hardcoded_val'"),
            "must include -X flags"
        );
    }

    // ── template_to_shell edge cases ──────────────────────────────────

    #[test]
    fn template_to_shell_empty_input() {
        assert_eq!(template_to_shell(""), "");
    }

    #[test]
    fn template_to_shell_adjacent_placeholders() {
        assert_eq!(template_to_shell("{{a}}{{b}}"), "$a$b");
    }

    #[test]
    fn template_to_shell_preserves_surrounding_text() {
        assert_eq!(
            template_to_shell("prefix-{{var}}-suffix"),
            "prefix-$var-suffix"
        );
    }
}
