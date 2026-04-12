//! `dockermint build` subcommand.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Args;

use crate::recipe::types::FlavorValue;

/// Arguments for the `build` subcommand.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Path to the recipe TOML file.
    #[arg(short, long)]
    pub recipe: PathBuf,

    /// Git tag to build.
    #[arg(short, long)]
    pub tag: String,

    /// Target platforms (e.g. `linux/amd64,linux/arm64`).
    #[arg(short, long, default_value = "linux/amd64")]
    pub platform: String,

    /// Flavor overrides as `key=value` pairs.
    ///
    /// Example: `--flavor db_backend=pebbledb --flavor
    /// build_tags=netgo,muslc`
    #[arg(short, long = "flavor", value_parser = parse_flavor)]
    pub flavors: Vec<(String, FlavorValue)>,

    /// Push the built image to the registry after building.
    #[arg(long, default_value_t = false)]
    pub push: bool,
}

impl BuildArgs {
    /// Convert the `--flavor` arguments into a [`HashMap`] for flavor
    /// resolution.
    pub fn flavor_overrides(&self) -> HashMap<String, FlavorValue> {
        self.flavors.iter().cloned().collect()
    }

    /// Parse the `--platform` argument into a list of platforms.
    pub fn platforms(&self) -> Vec<String> {
        self.platform
            .split(',')
            .map(|s| s.trim().to_owned())
            .collect()
    }
}

/// Parse a `key=value` string into a flavor override.
///
/// Values containing commas are treated as multi-value selections.
fn parse_flavor(s: &str) -> Result<(String, FlavorValue), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid flavor format: '{s}' (expected key=value)"))?;

    let value = if value.contains(',') {
        FlavorValue::Multiple(value.split(',').map(|v| v.trim().to_owned()).collect())
    } else {
        FlavorValue::Single(value.to_owned())
    };

    Ok((key.to_owned(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_flavor() {
        let (k, v) = parse_flavor("db_backend=pebbledb").unwrap();
        assert_eq!(k, "db_backend");
        assert_eq!(v, FlavorValue::Single("pebbledb".to_owned()));
    }

    #[test]
    fn parse_multi_flavor() {
        let (k, v) = parse_flavor("build_tags=netgo,muslc").unwrap();
        assert_eq!(k, "build_tags");
        assert_eq!(
            v,
            FlavorValue::Multiple(vec!["netgo".to_owned(), "muslc".to_owned(),])
        );
    }

    #[test]
    fn parse_missing_equals_fails() {
        assert!(parse_flavor("noequalssign").is_err());
    }
}
