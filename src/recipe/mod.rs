//! Recipe loading, flavor resolution, and validation.
//!
//! Recipes are TOML files in the `recipes/` directory.  This module
//! parses them, resolves flavor selections (CLI > config > defaults),
//! and validates compatibility.

pub mod host_vars;
pub mod types;
pub mod validation;

use std::collections::HashMap;
use std::path::Path;

use crate::builder::template::TemplateEngine;
use crate::config::types::RecipeFlavourOverride;
use crate::error::RecipeError;
use crate::recipe::types::{FlavorValue, Recipe, ResolvedRecipe, SelectedFlavours};

/// Maximum supported recipe schema version.
const MAX_SCHEMA_VERSION: u32 = 1;

/// Parse a recipe TOML file from disk.
///
/// # Arguments
///
/// * `path` - Path to the `.toml` recipe file
///
/// # Returns
///
/// The deserialized [`Recipe`].
///
/// # Errors
///
/// - [`RecipeError::ReadFile`] if the file cannot be read.
/// - [`RecipeError::Parse`] if TOML deserialization fails.
/// - [`RecipeError::UnsupportedSchema`] if the schema version exceeds
///   what this build supports.
pub fn load(path: &Path) -> Result<Recipe, RecipeError> {
    let contents = std::fs::read_to_string(path).map_err(|e| RecipeError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let recipe: Recipe = toml::from_str(&contents)?;

    if recipe.meta.schema_version > MAX_SCHEMA_VERSION {
        return Err(RecipeError::UnsupportedSchema(recipe.meta.schema_version));
    }

    Ok(recipe)
}

/// Load all recipes from a directory.
///
/// # Arguments
///
/// * `dir` - Directory containing `.toml` recipe files
///
/// # Returns
///
/// A map of recipe file stem to parsed [`Recipe`].
///
/// # Errors
///
/// Returns the first [`RecipeError`] encountered.
pub fn load_all(dir: &Path) -> Result<HashMap<String, Recipe>, RecipeError> {
    let mut recipes = HashMap::new();

    let entries = std::fs::read_dir(dir).map_err(|e| RecipeError::ReadFile {
        path: dir.to_path_buf(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| RecipeError::ReadFile {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "toml") {
            let stem = path
                .file_stem()
                .expect("file has extension so stem exists")
                .to_string_lossy()
                .into_owned();

            let recipe = load(&path)?;
            recipes.insert(stem, recipe);
        }
    }

    Ok(recipes)
}

/// Resolve flavor selections for a recipe using the priority chain:
/// CLI overrides > config.toml overrides > recipe defaults.
///
/// # Arguments
///
/// * `recipe` - Parsed recipe with available/default flavors
/// * `config_overrides` - Overrides from `config.toml` (may be `None`)
/// * `cli_overrides` - Overrides from CLI arguments (may be `None`)
///
/// # Returns
///
/// [`SelectedFlavours`] with fully resolved selections.
///
/// # Errors
///
/// - [`RecipeError::IncompatibleFlavour`] if a selected value is not in
///   the available set.
/// - [`RecipeError::UnknownFlavour`] if an override references a
///   non-existent dimension.
pub fn resolve_flavours(
    recipe: &Recipe,
    config_overrides: Option<&RecipeFlavourOverride>,
    cli_overrides: Option<&HashMap<String, FlavorValue>>,
) -> Result<SelectedFlavours, RecipeError> {
    let mut selections = HashMap::new();

    // Start with recipe defaults
    for (dim, default_val) in &recipe.flavours.default {
        selections.insert(dim.clone(), default_val.clone());
    }

    // Apply config.toml overrides
    if let Some(overrides) = config_overrides {
        for (dim, val) in &overrides.0 {
            if !recipe.flavours.available.contains_key(dim) {
                return Err(RecipeError::UnknownFlavour(dim.clone()));
            }
            selections.insert(dim.clone(), val.clone());
        }
    }

    // Apply CLI overrides (highest priority)
    if let Some(overrides) = cli_overrides {
        for (dim, val) in overrides {
            if !recipe.flavours.available.contains_key(dim) {
                return Err(RecipeError::UnknownFlavour(dim.clone()));
            }
            selections.insert(dim.clone(), val.clone());
        }
    }

    // Validate all selections against available options
    let selected = SelectedFlavours { selections };
    validation::validate_flavours(recipe, &selected)?;

    Ok(selected)
}

/// Resolve a recipe into a fully ready-to-build state.
///
/// Combines flavor resolution with profile variable injection.
///
/// # Arguments
///
/// * `recipe` - Parsed recipe
/// * `config_overrides` - Config flavor overrides
/// * `cli_overrides` - CLI flavor overrides
/// * `host_variables` - Variables from the host environment
///
/// # Returns
///
/// A [`ResolvedRecipe`] ready for the builder.
///
/// # Errors
///
/// Returns [`RecipeError`] on flavor incompatibility or unknown
/// dimensions.
pub fn resolve(
    recipe: Recipe,
    config_overrides: Option<&RecipeFlavourOverride>,
    cli_overrides: Option<&HashMap<String, FlavorValue>>,
    host_variables: &HashMap<String, String>,
) -> Result<ResolvedRecipe, RecipeError> {
    let selected_flavours = resolve_flavours(&recipe, config_overrides, cli_overrides)?;

    let mut variables = host_variables.clone();

    // Inject flavor selections as variables, expanding any
    // host-variable references (e.g. architecture = "{{HOST_ARCH}}")
    for (dim, val) in &selected_flavours.selections {
        match val {
            FlavorValue::Single(s) => {
                let expanded = TemplateEngine::render(s, &variables);
                variables.insert(dim.clone(), expanded);
            },
            FlavorValue::Multiple(v) => {
                let expanded: Vec<String> = v
                    .iter()
                    .map(|s| TemplateEngine::render(s, &variables))
                    .collect();
                variables.insert(dim.clone(), expanded.join(","));
            },
        }
    }

    // Inject header fields as variables
    variables.insert("binary_name".to_owned(), recipe.header.binary_name.clone());

    // Inject profile variables based on selected flavors
    for (profile_dim, options) in &recipe.profiles {
        if let Some(FlavorValue::Single(selected)) = selected_flavours.selections.get(profile_dim)
            && let Some(profile_vars) = options.get(selected)
        {
            for (k, v) in profile_vars {
                variables.insert(k.clone(), v.clone());
            }
        }
    }

    Ok(ResolvedRecipe {
        recipe,
        selected_flavours,
        resolved_variables: variables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_cosmos_recipe() {
        let path = Path::new("recipes/cosmos-gaiad.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            assert_eq!(recipe.header.name, "Cosmos");
            assert_eq!(recipe.header.binary_name, "gaiad");
            assert_eq!(recipe.meta.schema_version, 1);
        }
    }

    #[test]
    fn load_kyve_recipe() {
        let path = Path::new("recipes/kyve-kyved.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            assert_eq!(recipe.header.name, "Kyve");
            assert_eq!(recipe.header.binary_name, "kyved");
            assert!(!recipe.profiles.is_empty(), "kyve has profiles");
            assert!(
                recipe.profiles.contains_key("network"),
                "kyve has network profiles"
            );
        }
    }

    #[test]
    fn resolve_uses_defaults_when_no_overrides() {
        let path = Path::new("recipes/cosmos-gaiad.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            let selected = resolve_flavours(&recipe, None, None).expect("should resolve");
            assert!(selected.selections.contains_key("db_backend"));
        }
    }

    #[test]
    fn resolve_expands_host_arch_in_flavors() {
        let path = Path::new("recipes/cosmos-gaiad.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            let mut host_vars = HashMap::new();
            host_vars.insert("HOST_ARCH".to_owned(), "x86_64".to_owned());

            let resolved = resolve(recipe, None, None, &host_vars).expect("should resolve");

            let arch = &resolved.resolved_variables["architecture"];
            assert_eq!(arch, "x86_64", "{{{{HOST_ARCH}}}} must expand");
        }
    }

    #[test]
    fn resolve_injects_profile_variables() {
        let path = Path::new("recipes/kyve-kyved.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            let host_vars = HashMap::new();

            let resolved = resolve(recipe, None, None, &host_vars).expect("should resolve");

            // Default network is mainnet -> denom = "ukyve"
            assert_eq!(
                resolved.resolved_variables.get("denom"),
                Some(&"ukyve".to_owned()),
                "mainnet profile should inject denom"
            );
        }
    }

    #[test]
    fn resolve_injects_binary_name() {
        let path = Path::new("recipes/cosmos-gaiad.toml");
        if path.exists() {
            let recipe = load(path).expect("should parse");
            let resolved = resolve(recipe, None, None, &HashMap::new()).expect("should resolve");
            assert_eq!(resolved.resolved_variables["binary_name"], "gaiad");
        }
    }

    #[test]
    fn load_all_finds_recipes() {
        let dir = Path::new("recipes");
        if dir.exists() {
            let recipes = load_all(dir).expect("should load");
            assert!(
                recipes.contains_key("cosmos-gaiad"),
                "should find cosmos-gaiad"
            );
            assert!(recipes.contains_key("kyve-kyved"), "should find kyve-kyved");
        }
    }
}
