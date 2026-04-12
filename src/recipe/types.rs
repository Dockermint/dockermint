//! Data types representing a Dockermint recipe TOML and its resolved state.
//!
//! A **recipe** is the complete build specification for a single blockchain
//! binary.  It declares available flavors, build variables, Dockerfile
//! fragments, copy rules, labels, and image tags.
//!
//! Key terminology:
//! - **Host variable** (`{{UPPERCASE}}`): resolved from the host environment
//!   at Dockermint startup (e.g. `{{HOST_ARCH}}`).
//! - **Build variable** (`{{lowercase}}`): resolved dynamically during the
//!   build from shell commands or profile tables.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ===========================================================================
// Top-level Recipe
// ===========================================================================

/// A fully deserialized recipe TOML file.
///
/// # Examples
///
/// ```no_run
/// let contents = std::fs::read_to_string("recipes/cosmos-gaiad.toml")?;
/// let recipe: dockermint::recipe::types::Recipe = toml::from_str(&contents)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Recipe schema and version constraints.
    pub meta: RecipeMeta,

    /// Project identity (name, repo, binary).
    pub header: RecipeHeader,

    /// Available and default flavor dimensions.
    pub flavours: RecipeFlavours,

    /// Container image used for scrapping (cloning) the source repo.
    pub scrapper: RecipeScrapper,

    /// Shell commands whose stdout becomes a named build variable.
    #[serde(default)]
    pub variables: HashMap<String, VariableDefinition>,

    /// Platform-specific package installation commands for the builder
    /// stage.
    #[serde(default)]
    pub builder: RecipeBuilderInstall,

    /// Conditional Dockerfile instructions executed before the build.
    #[serde(default)]
    pub pre_build: Vec<PreBuildStep>,

    /// Build environment, linker configuration, and build path.
    pub build: RecipeBuild,

    /// Per-user-type configuration (e.g. `[user.dockermint]`).
    #[serde(default)]
    pub user: HashMap<String, UserConfig>,

    /// Files to copy from builder to runner stage.
    pub copy: RecipeCopySection,

    /// Ports to expose in the final image.
    pub expose: RecipeExpose,

    /// OCI labels applied to the final image.
    #[serde(default)]
    pub labels: HashMap<String, String>,

    /// Image tag template.
    pub image: RecipeImage,

    /// Optional profiles that inject extra variables based on a flavor
    /// selection (e.g. `[profiles.network.mainnet]`).
    #[serde(default)]
    pub profiles: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

// ===========================================================================
// Recipe sections
// ===========================================================================

/// `[meta]` -- schema version and minimum tool version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeMeta {
    /// Schema version of this recipe file.
    pub schema_version: u32,

    /// Minimum Dockermint version required to process this recipe.
    pub min_dockermint_version: String,
}

/// `[header]` -- project identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeHeader {
    /// Human-readable project name (e.g. "Cosmos").
    pub name: String,

    /// Git repository URL.
    pub repo: String,

    /// Build system type (e.g. "golang").
    #[serde(rename = "type")]
    pub build_type: String,

    /// Name of the produced binary (e.g. "gaiad").
    pub binary_name: String,

    /// Glob patterns for tags to include (comma-separated).
    #[serde(default)]
    pub include_patterns: String,

    /// Glob patterns for tags to exclude (comma-separated).
    #[serde(default)]
    pub exclude_patterns: String,
}

/// `[flavours]` -- available options and defaults for each flavor
/// dimension.
///
/// Flavor dimensions are **recipe-specific** and dynamic.  A Cosmos recipe
/// may define `architecture`, `db_backend`, `binary_type`, etc., while a
/// KYVE recipe adds `network`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeFlavours {
    /// Maps a flavor dimension name to its valid options.
    ///
    /// Example: `"db_backend" -> ["goleveldb", "pebbledb"]`
    pub available: HashMap<String, Vec<String>>,

    /// Maps a flavor dimension name to its default selection.
    ///
    /// The value is either a single string or an array of strings.
    pub default: HashMap<String, FlavorValue>,
}

/// A flavor value -- either a single selection or multiple selections.
///
/// Serialized as an untagged enum so TOML like
/// `db_backend = "goleveldb"` and `build_tags = ["netgo", "muslc"]`
/// both parse correctly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FlavorValue {
    /// A single selected option (e.g. `db_backend = "goleveldb"`).
    Single(String),

    /// Multiple selected options (e.g. `build_tags = ["netgo", "muslc"]`).
    Multiple(Vec<String>),
}

/// `[scrapper]` -- how to clone the source repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeScrapper {
    /// Docker image used for the scrapper stage.
    pub image: String,

    /// Shell command to install dependencies inside the scrapper image.
    #[serde(default)]
    pub install: String,

    /// Environment variables forwarded into the scrapper container.
    /// May contain template variables like `{{GH_USER}}`.
    #[serde(default)]
    pub env: Vec<String>,

    /// Clone method (e.g. `"try-authenticated-clone"`).
    pub method: String,

    /// Working directory inside the scrapper container.
    pub directory: String,
}

/// A build variable resolved by executing a shell command.
///
/// Appears in `[variables]` as e.g.
/// `repo_commit = { shell = "git log -1 --format='%H'" }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableDefinition {
    /// Shell command whose stdout becomes the variable value.
    pub shell: String,
}

/// `[builder]` -- platform-specific installation commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipeBuilderInstall {
    /// Maps a distribution family (e.g. `"alpine"`, `"ubuntu"`) to the
    /// shell command that installs build dependencies.
    #[serde(default)]
    pub install: HashMap<String, String>,
}

/// A conditional step executed before the main build.
///
/// Appears as `[[pre_build]]` array entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreBuildStep {
    /// Flavor value that activates this step (e.g. `"static"`).
    pub condition: String,

    /// Dockerfile instruction (e.g. `"ADD"`, `"RUN"`, `"COPY"`).
    pub instruction: String,

    /// Source argument for the instruction.
    #[serde(default)]
    pub source: String,

    /// Destination argument for the instruction.
    #[serde(default)]
    pub dest: String,
}

/// `[build]` -- environment, linker flags, and build path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeBuild {
    /// Environment variables set during the build (e.g. `CGO_ENABLED`).
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Linker configuration.
    pub linker: LinkerConfig,

    /// Path to the build target.
    pub path: BuildPath,
}

/// `[build.linker]` -- linker flags and embedded version variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkerConfig {
    /// Maps a binary_type flavor (e.g. `"dynamic"`, `"static"`) to its
    /// linker flag string.
    pub flags: HashMap<String, String>,

    /// Maps a Go import path to the value to embed via `-X`.
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

/// `[build.path]` -- location of the build entry point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildPath {
    /// Path to the package to build (may contain template variables).
    pub path: String,
}

/// Per-user-type configuration (e.g. `[user.dockermint]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Username inside the container.
    pub username: String,
    /// Numeric UID.
    pub uid: u32,
    /// Numeric GID.
    pub gid: u32,
}

// ---------------------------------------------------------------------------
// Copy section
// ---------------------------------------------------------------------------

/// `[copy]` -- files to copy from the builder stage to the runner stage.
///
/// Top-level entries are always copied.  Sub-tables keyed by a
/// `binary_type` flavor value (e.g. `[copy.dynamic]`) are conditional.
///
/// Because the keys are heterogeneous (file paths AND sub-table names),
/// we deserialize the raw TOML table and provide accessor methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecipeCopySection(pub toml::Table);

/// A single copy entry: `"/src/path" = { dest = "/dst", type = "kind" }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyEntry {
    /// Destination path inside the runner image.
    pub dest: String,

    /// Kind of artifact: `"entrypoint"`, `"dyn-library"`, etc.
    #[serde(rename = "type")]
    pub copy_type: String,
}

impl RecipeCopySection {
    /// Return entries that are **always** copied (top-level keys whose
    /// values are inline tables with `dest` and `type`).
    ///
    /// # Errors
    ///
    /// Returns `None` for keys whose value is a sub-table (conditional
    /// copies).
    pub fn always_entries(&self) -> impl Iterator<Item = (&str, CopyEntry)> {
        self.0.iter().filter_map(|(k, v)| {
            // Sub-tables (like [copy.dynamic]) contain nested tables,
            // not {dest, type} pairs.
            if v.is_table() {
                if let Some(tbl) = v.as_table()
                    && tbl.contains_key("dest")
                {
                    let entry: CopyEntry = v.clone().try_into().ok()?;
                    return Some((k.as_str(), entry));
                }
                None
            } else {
                // Inline table
                let entry: CopyEntry = v.clone().try_into().ok()?;
                Some((k.as_str(), entry))
            }
        })
    }

    /// Return conditional copy entries for a given `binary_type` flavor.
    ///
    /// # Arguments
    ///
    /// * `flavor_value` - The `binary_type` value (e.g. `"dynamic"`)
    ///
    /// # Returns
    ///
    /// An iterator of `(source_path, CopyEntry)` pairs for the condition,
    /// or an empty iterator if no conditional section exists.
    pub fn conditional_entries(&self, flavor_value: &str) -> Vec<(String, CopyEntry)> {
        let Some(toml::Value::Table(sub)) = self.0.get(flavor_value) else {
            return Vec::new();
        };
        sub.iter()
            .filter_map(|(k, v)| {
                let entry: CopyEntry = v.clone().try_into().ok()?;
                Some((k.clone(), entry))
            })
            .collect()
    }
}

/// `[expose]` -- ports to expose in the final image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeExpose {
    /// List of ports with descriptions.
    pub ports: Vec<PortEntry>,
}

/// A single exposed port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortEntry {
    /// Port number.
    pub port: u16,
    /// Human-readable purpose.
    pub description: String,
}

/// `[image]` -- tag template for the final Docker image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeImage {
    /// Tag template with `{{variables}}` (e.g.
    /// `"cosmos-gaiad-{{db_backend}}:{{SEMVER_TAG}}-{{running_env}}"`).
    pub tag: String,
}

// ===========================================================================
// Resolved types (post-parsing)
// ===========================================================================

/// Flavor selections resolved from the priority chain:
/// CLI args > config.toml > recipe defaults.
#[derive(Debug, Clone, Default)]
pub struct SelectedFlavours {
    /// Maps each flavor dimension to its resolved value(s).
    pub selections: HashMap<String, FlavorValue>,
}

impl SelectedFlavours {
    /// Get the single-value selection for a flavor dimension.
    ///
    /// # Arguments
    ///
    /// * `key` - Flavor dimension name
    ///
    /// # Returns
    ///
    /// `Some(&str)` if the dimension exists and is a single value,
    /// `None` otherwise.
    pub fn get_single(&self, key: &str) -> Option<&str> {
        match self.selections.get(key)? {
            FlavorValue::Single(s) => Some(s.as_str()),
            FlavorValue::Multiple(_) => None,
        }
    }

    /// Get the multi-value selection for a flavor dimension.
    ///
    /// # Arguments
    ///
    /// * `key` - Flavor dimension name
    ///
    /// # Returns
    ///
    /// `Some(&[String])` if the dimension exists and is multi-value,
    /// `None` otherwise.
    pub fn get_multiple(&self, key: &str) -> Option<&[String]> {
        match self.selections.get(key)? {
            FlavorValue::Single(_) => None,
            FlavorValue::Multiple(v) => Some(v.as_slice()),
        }
    }

    /// Check whether any active flavor selection contains `value`.
    ///
    /// Used to evaluate `[[pre_build]]` conditions and conditional
    /// `[copy.*]` sub-tables.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to search for across all dimensions
    ///
    /// # Returns
    ///
    /// `true` if any single-value selection equals `value` or any
    /// multi-value selection contains `value`.
    pub fn has_value(&self, value: &str) -> bool {
        self.selections.values().any(|v| match v {
            FlavorValue::Single(s) => s == value,
            FlavorValue::Multiple(vs) => vs.iter().any(|s| s == value),
        })
    }
}

/// A recipe with all flavors resolved and variables ready for template
/// expansion.
#[derive(Debug, Clone)]
pub struct ResolvedRecipe {
    /// The original parsed recipe.
    pub recipe: Recipe,

    /// Flavor selections after applying the priority chain.
    pub selected_flavours: SelectedFlavours,

    /// Fully resolved template variables (both host and build variables).
    pub resolved_variables: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: deserialize a FlavorValue embedded in a TOML key.
    fn fv(toml_fragment: &str) -> FlavorValue {
        #[derive(Deserialize)]
        struct Wrapper {
            v: FlavorValue,
        }
        let w: Wrapper = toml::from_str(toml_fragment).expect("parse");
        w.v
    }

    #[test]
    fn flavor_value_deserializes_single() {
        let val = fv(r#"v = "goleveldb""#);
        assert_eq!(val, FlavorValue::Single("goleveldb".to_owned()));
    }

    #[test]
    fn flavor_value_deserializes_multiple() {
        let val = fv(r#"v = ["netgo", "muslc"]"#);
        assert_eq!(
            val,
            FlavorValue::Multiple(vec!["netgo".to_owned(), "muslc".to_owned(),])
        );
    }

    #[test]
    fn selected_flavours_accessors() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "db_backend".to_owned(),
            FlavorValue::Single("goleveldb".to_owned()),
        );
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Multiple(vec!["netgo".to_owned(), "muslc".to_owned()]),
        );

        assert_eq!(sf.get_single("db_backend"), Some("goleveldb"));
        assert_eq!(sf.get_single("build_tags"), None);
        assert_eq!(sf.get_multiple("db_backend"), None);
        assert_eq!(
            sf.get_multiple("build_tags"),
            Some(vec!["netgo".to_owned(), "muslc".to_owned()].as_slice())
        );
    }
}
