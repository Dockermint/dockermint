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
        FlavorValue::Single(value.trim().to_owned())
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
    fn parse_single_flavor_trims_whitespace() {
        let (k, v) = parse_flavor("db_backend= pebbledb ").unwrap();
        assert_eq!(k, "db_backend");
        assert_eq!(v, FlavorValue::Single("pebbledb".to_owned()));
    }

    #[test]
    fn parse_missing_equals_fails() {
        assert!(parse_flavor("noequalssign").is_err());
    }

    // -- additional tests for mutation coverage --

    #[test]
    fn parse_flavor_empty_value() {
        let (k, v) = parse_flavor("key=").unwrap();
        assert_eq!(k, "key");
        assert_eq!(v, FlavorValue::Single(String::new()));
    }

    #[test]
    fn parse_flavor_empty_key() {
        let (k, v) = parse_flavor("=value").unwrap();
        assert_eq!(k, "");
        assert_eq!(v, FlavorValue::Single("value".to_owned()));
    }

    #[test]
    fn parse_flavor_multiple_equals() {
        let (k, v) = parse_flavor("key=val=ue").unwrap();
        assert_eq!(k, "key");
        // split_once splits at first '=', so value is "val=ue"
        // val=ue does not contain a bare comma, so Single
        assert_eq!(v, FlavorValue::Single("val=ue".to_owned()));
    }

    #[test]
    fn parse_flavor_multi_trims_whitespace() {
        let (k, v) = parse_flavor("tags= netgo , muslc , ledger ").unwrap();
        assert_eq!(k, "tags");
        assert_eq!(
            v,
            FlavorValue::Multiple(vec![
                "netgo".to_owned(),
                "muslc".to_owned(),
                "ledger".to_owned(),
            ])
        );
    }

    #[test]
    fn parse_flavor_error_message_contains_input() {
        let err = parse_flavor("noeq").unwrap_err();
        assert!(
            err.contains("noeq"),
            "error should contain the invalid input: {err}"
        );
    }

    #[test]
    fn platforms_single_default() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64".to_owned(),
            flavors: vec![],
            push: false,
        };
        let platforms = args.platforms();
        assert_eq!(platforms, vec!["linux/amd64"]);
    }

    #[test]
    fn platforms_multiple() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64,linux/arm64".to_owned(),
            flavors: vec![],
            push: false,
        };
        let platforms = args.platforms();
        assert_eq!(platforms, vec!["linux/amd64", "linux/arm64"]);
    }

    #[test]
    fn platforms_trims_whitespace() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: " linux/amd64 , linux/arm64 ".to_owned(),
            flavors: vec![],
            push: false,
        };
        let platforms = args.platforms();
        assert_eq!(platforms, vec!["linux/amd64", "linux/arm64"]);
    }

    #[test]
    fn platforms_three_entries() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64,linux/arm64,linux/arm/v7".to_owned(),
            flavors: vec![],
            push: false,
        };
        let platforms = args.platforms();
        assert_eq!(platforms.len(), 3);
        assert_eq!(platforms[2], "linux/arm/v7");
    }

    #[test]
    fn flavor_overrides_empty_when_no_flavors() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64".to_owned(),
            flavors: vec![],
            push: false,
        };
        let overrides = args.flavor_overrides();
        assert!(overrides.is_empty());
    }

    #[test]
    fn flavor_overrides_collects_pairs() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64".to_owned(),
            flavors: vec![
                (
                    "db_backend".to_owned(),
                    FlavorValue::Single("pebbledb".to_owned()),
                ),
                (
                    "tags".to_owned(),
                    FlavorValue::Multiple(vec!["netgo".to_owned(), "muslc".to_owned()]),
                ),
            ],
            push: false,
        };
        let overrides = args.flavor_overrides();
        assert_eq!(overrides.len(), 2);
        assert_eq!(
            overrides.get("db_backend"),
            Some(&FlavorValue::Single("pebbledb".to_owned()))
        );
    }

    #[test]
    fn flavor_overrides_last_wins_on_duplicate_key() {
        let args = BuildArgs {
            recipe: PathBuf::from("test.toml"),
            tag: "v1.0.0".to_owned(),
            platform: "linux/amd64".to_owned(),
            flavors: vec![
                ("db".to_owned(), FlavorValue::Single("first".to_owned())),
                ("db".to_owned(), FlavorValue::Single("second".to_owned())),
            ],
            push: false,
        };
        let overrides = args.flavor_overrides();
        assert_eq!(overrides.len(), 1);
        // HashMap from iter: last value wins
        assert_eq!(overrides["db"], FlavorValue::Single("second".to_owned()));
    }
}
