//! Flavor compatibility validation.
//!
//! Ensures that every selected flavor value is listed in the recipe's
//! `[flavours.available]` section.

use crate::error::RecipeError;
use crate::recipe::types::{FlavorValue, Recipe, SelectedFlavours};

/// Validate that all selected flavors are compatible with the recipe.
///
/// Each selected value must appear in the corresponding
/// `flavours.available` list.
///
/// # Arguments
///
/// * `recipe` - The parsed recipe with available options
/// * `selected` - The resolved flavor selections to validate
///
/// # Errors
///
/// - [`RecipeError::IncompatibleFlavour`] if a value is not in the
///   available set.
/// - [`RecipeError::UnknownFlavour`] if a dimension does not exist.
pub fn validate_flavours(recipe: &Recipe, selected: &SelectedFlavours) -> Result<(), RecipeError> {
    for (dim, val) in &selected.selections {
        let available = recipe
            .flavours
            .available
            .get(dim)
            .ok_or_else(|| RecipeError::UnknownFlavour(dim.clone()))?;

        match val {
            FlavorValue::Single(s) => {
                // Skip host-variable defaults like "{{HOST_ARCH}}"
                if !s.starts_with("{{") && !available.contains(s) {
                    return Err(RecipeError::IncompatibleFlavour {
                        flavour: dim.clone(),
                        value: s.clone(),
                    });
                }
            },
            FlavorValue::Multiple(values) => {
                for v in values {
                    if !v.starts_with("{{") && !available.contains(v) {
                        return Err(RecipeError::IncompatibleFlavour {
                            flavour: dim.clone(),
                            value: v.clone(),
                        });
                    }
                }
            },
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::recipe::types::{
        BuildPath, LinkerConfig, RecipeBuild, RecipeBuilderInstall, RecipeCopySection,
        RecipeExpose, RecipeFlavours, RecipeHeader, RecipeImage, RecipeMeta, RecipeScrapper,
    };

    fn minimal_recipe(
        available: HashMap<String, Vec<String>>,
        default: HashMap<String, FlavorValue>,
    ) -> Recipe {
        Recipe {
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
            flavours: RecipeFlavours { available, default },
            scrapper: RecipeScrapper {
                image: String::new(),
                install: String::new(),
                env: Vec::new(),
                method: String::new(),
                directory: String::new(),
            },
            variables: HashMap::new(),
            builder: RecipeBuilderInstall::default(),
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
        }
    }

    #[test]
    fn valid_single_selection_passes() {
        let mut available = HashMap::new();
        available.insert(
            "db_backend".to_owned(),
            vec!["goleveldb".to_owned(), "pebbledb".to_owned()],
        );

        let recipe = minimal_recipe(available, HashMap::new());

        let mut selections = HashMap::new();
        selections.insert(
            "db_backend".to_owned(),
            FlavorValue::Single("pebbledb".to_owned()),
        );
        let selected = SelectedFlavours { selections };

        assert!(validate_flavours(&recipe, &selected).is_ok());
    }

    #[test]
    fn invalid_selection_fails() {
        let mut available = HashMap::new();
        available.insert("db_backend".to_owned(), vec!["goleveldb".to_owned()]);

        let recipe = minimal_recipe(available, HashMap::new());

        let mut selections = HashMap::new();
        selections.insert(
            "db_backend".to_owned(),
            FlavorValue::Single("rocksdb".to_owned()),
        );
        let selected = SelectedFlavours { selections };

        let err = validate_flavours(&recipe, &selected).unwrap_err();
        assert!(
            matches!(err, RecipeError::IncompatibleFlavour { .. }),
            "expected IncompatibleFlavour, got: {err:?}"
        );
    }

    #[test]
    fn template_variable_default_skips_validation() {
        let mut available = HashMap::new();
        available.insert(
            "architecture".to_owned(),
            vec!["x86_64".to_owned(), "aarch64".to_owned()],
        );

        let recipe = minimal_recipe(available, HashMap::new());

        let mut selections = HashMap::new();
        selections.insert(
            "architecture".to_owned(),
            FlavorValue::Single("{{HOST_ARCH}}".to_owned()),
        );
        let selected = SelectedFlavours { selections };

        assert!(
            validate_flavours(&recipe, &selected).is_ok(),
            "template variables should be allowed"
        );
    }
}
