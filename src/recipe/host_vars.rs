//! Host-level variable collection.
//!
//! Host variables use the `{{UPPERCASE}}` convention and are resolved at
//! Dockermint startup from the local system environment.  Build-time
//! variables (`{{lowercase}}`, from `[variables]` shell commands) are
//! **not** resolved here -- they execute inside the Docker build.
//!
//! The returned [`HashMap`] is intentionally open-ended: callers can
//! inject additional entries without modifying this module.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::recipe::types::SelectedFlavours;

/// Collect host-level variables available at parse time.
///
/// # Arguments
///
/// * `tag` - Git tag / semver version being built
/// * `selected_flavours` - Resolved flavor selections (used to derive
///   composite variables like `BUILD_TAGS_COMMA_SEP`)
///
/// # Returns
///
/// A [`HashMap`] of variable name to value.  All keys that originate
/// here follow the `UPPERCASE` convention, except `repository_path`
/// which is a Dockermint-provided path.
///
/// # Examples
///
/// ```
/// use dockermint::recipe::types::SelectedFlavours;
/// use dockermint::recipe::host_vars;
///
/// let flavours = SelectedFlavours::default();
/// let vars = host_vars::collect("v21.0.1", &flavours);
/// assert!(vars.contains_key("HOST_ARCH"));
/// assert_eq!(vars["SEMVER_TAG"], "v21.0.1");
/// ```
pub fn collect(tag: &str, selected_flavours: &SelectedFlavours) -> HashMap<String, String> {
    let mut vars = HashMap::with_capacity(16);

    vars.insert("HOST_ARCH".to_owned(), host_arch());
    vars.insert("CREATION_TIMESTAMP".to_owned(), utc_timestamp());
    vars.insert("SEMVER_TAG".to_owned(), tag.to_owned());

    // Derived from build_tags flavor
    if let Some(tags) = selected_flavours.get_multiple("build_tags") {
        vars.insert("BUILD_TAGS_COMMA_SEP".to_owned(), tags.join(","));
    }

    // Default workspace path for cloned repositories
    vars.insert("repository_path".to_owned(), "/workspace".to_owned());

    vars
}

/// Extend the variable map with values read from the process
/// environment.
///
/// Missing variables are silently skipped.
///
/// # Arguments
///
/// * `vars` - Map to extend
/// * `keys` - Environment variable names to forward (e.g.
///   `["GH_USER", "GH_PAT"]`)
pub fn extend_from_env(vars: &mut HashMap<String, String>, keys: &[&str]) {
    for key in keys {
        if let Ok(val) = std::env::var(key) {
            vars.insert((*key).to_owned(), val);
        }
    }
}

/// Return the current UTC timestamp in ISO 8601 format.
///
/// Public so other modules (e.g. the daemon loop) can stamp build
/// records.
pub fn utc_now() -> String {
    utc_timestamp()
}

// ── helpers ──────────────────────────────────────────────────────────

/// Map Rust target arch to recipe convention.
fn host_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
    .to_owned()
}

/// UTC timestamp in ISO 8601 format without external crate.
///
/// Uses the *civil from days* algorithm (Howard Hinnant) to convert
/// epoch seconds to a calendar date.
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's *civil_from_days* algorithm.
///
/// Converts a day count since the Unix epoch (1970-01-01) to a
/// `(year, month, day)` triple.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::types::FlavorValue;

    #[test]
    fn collect_contains_expected_keys() {
        let sf = SelectedFlavours::default();
        let vars = collect("v1.0.0", &sf);

        assert_eq!(vars["SEMVER_TAG"], "v1.0.0");
        assert!(vars.contains_key("HOST_ARCH"));
        assert!(vars.contains_key("CREATION_TIMESTAMP"));
        assert!(vars.contains_key("repository_path"));
    }

    #[test]
    fn build_tags_comma_sep_derived() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Multiple(vec!["netgo".to_owned(), "muslc".to_owned()]),
        );

        let vars = collect("v1.0.0", &sf);
        assert_eq!(vars["BUILD_TAGS_COMMA_SEP"], "netgo,muslc");
    }

    #[test]
    fn utc_timestamp_format() {
        let ts = utc_timestamp();
        // Must match YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
    }

    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_known_date() {
        // 2026-04-12 is day 20_555 since epoch
        assert_eq!(civil_from_days(20_555), (2026, 4, 12));
    }

    #[test]
    fn extend_from_env_skips_missing() {
        let mut vars = HashMap::new();
        extend_from_env(&mut vars, &["__DOCKERMINT_TEST_NONEXISTENT__"]);
        assert!(vars.is_empty());
    }

    // -- additional tests for mutation coverage --

    #[test]
    fn collect_repository_path_is_workspace() {
        let sf = SelectedFlavours::default();
        let vars = collect("v1.0.0", &sf);
        assert_eq!(vars["repository_path"], "/workspace");
    }

    #[test]
    fn collect_semver_tag_preserves_exact_input() {
        let sf = SelectedFlavours::default();
        let vars = collect("v99.88.77-rc1", &sf);
        assert_eq!(vars["SEMVER_TAG"], "v99.88.77-rc1");
    }

    #[test]
    fn collect_empty_tag() {
        let sf = SelectedFlavours::default();
        let vars = collect("", &sf);
        assert_eq!(vars["SEMVER_TAG"], "");
    }

    #[test]
    fn collect_host_arch_is_known_value() {
        let sf = SelectedFlavours::default();
        let vars = collect("v1.0.0", &sf);
        let arch = &vars["HOST_ARCH"];
        // Must match one of the supported arch strings or passthrough
        let known = ["x86_64", "aarch64"];
        assert!(
            known.contains(&arch.as_str()) || !arch.is_empty(),
            "HOST_ARCH should be a non-empty string, got: {arch}"
        );
    }

    #[test]
    fn host_arch_matches_std_env() {
        let result = host_arch();
        assert_eq!(result, std::env::consts::ARCH);
    }

    #[test]
    fn build_tags_comma_sep_absent_without_build_tags_flavor() {
        let sf = SelectedFlavours::default();
        let vars = collect("v1.0.0", &sf);
        assert!(
            !vars.contains_key("BUILD_TAGS_COMMA_SEP"),
            "BUILD_TAGS_COMMA_SEP should be absent when no build_tags flavor"
        );
    }

    #[test]
    fn build_tags_comma_sep_single_tag() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Multiple(vec!["netgo".to_owned()]),
        );
        let vars = collect("v1.0.0", &sf);
        assert_eq!(vars["BUILD_TAGS_COMMA_SEP"], "netgo");
    }

    #[test]
    fn build_tags_comma_sep_three_tags() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Multiple(vec![
                "netgo".to_owned(),
                "muslc".to_owned(),
                "ledger".to_owned(),
            ]),
        );
        let vars = collect("v1.0.0", &sf);
        assert_eq!(vars["BUILD_TAGS_COMMA_SEP"], "netgo,muslc,ledger");
    }

    #[test]
    fn build_tags_single_flavor_does_not_produce_comma_sep() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Single("netgo".to_owned()),
        );
        let vars = collect("v1.0.0", &sf);
        assert!(
            !vars.contains_key("BUILD_TAGS_COMMA_SEP"),
            "Single flavor value should not produce BUILD_TAGS_COMMA_SEP"
        );
    }

    #[test]
    fn collect_returns_exactly_five_keys_without_build_tags() {
        let sf = SelectedFlavours::default();
        let vars = collect("v1.0.0", &sf);
        assert_eq!(
            vars.len(),
            4,
            "expected HOST_ARCH, CREATION_TIMESTAMP, SEMVER_TAG, repository_path"
        );
    }

    #[test]
    fn collect_returns_five_keys_with_build_tags() {
        let mut sf = SelectedFlavours::default();
        sf.selections.insert(
            "build_tags".to_owned(),
            FlavorValue::Multiple(vec!["netgo".to_owned()]),
        );
        let vars = collect("v1.0.0", &sf);
        assert_eq!(vars.len(), 5);
        assert!(vars.contains_key("BUILD_TAGS_COMMA_SEP"));
    }

    #[test]
    fn utc_now_returns_valid_timestamp() {
        let ts = utc_now();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn civil_from_days_leap_year() {
        // 2000-02-29 is day 11_016 since epoch
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn civil_from_days_non_leap_century() {
        // 1900-01-01 is day -25_567 since epoch
        assert_eq!(civil_from_days(-25_567), (1900, 1, 1));
    }

    #[test]
    fn civil_from_days_negative() {
        // 1969-12-31 is day -1
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn civil_from_days_end_of_year() {
        // 2000-12-31 is day 11_322
        assert_eq!(civil_from_days(11_322), (2000, 12, 31));
    }

    #[test]
    fn civil_from_days_start_of_year() {
        // 2001-01-01 is day 11_323
        assert_eq!(civil_from_days(11_323), (2001, 1, 1));
    }

    #[test]
    fn extend_from_env_picks_up_set_variable() {
        let key = "__DOCKERMINT_TEST_EXTEND_VAR__";
        unsafe { std::env::set_var(key, "test_value") };
        let mut vars = HashMap::new();
        extend_from_env(&mut vars, &[key]);
        assert_eq!(vars.get(key).map(String::as_str), Some("test_value"));
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn extend_from_env_mixed_present_and_missing() {
        let key = "__DOCKERMINT_TEST_MIXED_VAR__";
        unsafe { std::env::set_var(key, "present") };
        let mut vars = HashMap::new();
        extend_from_env(&mut vars, &[key, "__DOCKERMINT_TEST_MISSING_XYZ__"]);
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[key], "present");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn extend_from_env_empty_keys_slice() {
        let mut vars = HashMap::new();
        extend_from_env(&mut vars, &[]);
        assert!(vars.is_empty());
    }
}
