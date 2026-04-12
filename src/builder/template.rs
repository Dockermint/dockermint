//! Template variable interpolation engine.
//!
//! Replaces `{{VARIABLE}}` placeholders in strings with their resolved
//! values.  Host variables use `{{UPPERCASE}}` and build variables use
//! `{{lowercase}}`, but the engine treats them uniformly -- the caller
//! is responsible for populating the variable map correctly.

use std::collections::HashMap;

/// Stateless template engine for `{{variable}}` interpolation.
///
/// All methods are associated functions (no `&self`) because the engine
/// carries no state.
pub struct TemplateEngine;

impl TemplateEngine {
    /// Replace all `{{key}}` occurrences in `template` with values from
    /// `vars`.
    ///
    /// Unknown variables are left as-is so downstream stages can detect
    /// unresolved placeholders.
    ///
    /// # Arguments
    ///
    /// * `template` - Input string with `{{placeholders}}`
    /// * `vars` - Variable name -> value map
    ///
    /// # Returns
    ///
    /// The expanded string.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use dockermint::builder::template::TemplateEngine;
    ///
    /// let mut vars = HashMap::new();
    /// vars.insert("HOST_ARCH".to_owned(), "x86_64".to_owned());
    /// vars.insert("db_backend".to_owned(), "goleveldb".to_owned());
    ///
    /// let result = TemplateEngine::render(
    ///     "arch={{HOST_ARCH}}, db={{db_backend}}",
    ///     &vars,
    /// );
    /// assert_eq!(result, "arch=x86_64, db=goleveldb");
    /// ```
    pub fn render(template: &str, vars: &HashMap<String, String>) -> String {
        let mut result = String::with_capacity(template.len());
        let mut rest = template;

        while let Some(open) = rest.find("{{") {
            // Emit everything before the opening braces.
            result.push_str(&rest[..open]);

            let after_open = &rest[open + 2..];
            if let Some(close) = after_open.find("}}") {
                let key = &after_open[..close];
                if let Some(val) = vars.get(key) {
                    result.push_str(val);
                } else {
                    result.push_str("{{");
                    result.push_str(key);
                    result.push_str("}}");
                }
                rest = &after_open[close + 2..];
            } else {
                // Unclosed -- emit literally and stop scanning.
                result.push_str("{{");
                result.push_str(after_open);
                return result;
            }
        }

        // Emit any remaining text after the last placeholder.
        result.push_str(rest);
        result
    }

    /// Check whether a string contains any unresolved `{{placeholders}}`.
    ///
    /// # Arguments
    ///
    /// * `s` - String to check
    ///
    /// # Returns
    ///
    /// A vector of unresolved variable names (may be empty).
    pub fn unresolved_vars(s: &str) -> Vec<String> {
        let mut vars = Vec::new();
        let mut rest = s;

        while let Some(start) = rest.find("{{") {
            let after_open = &rest[start + 2..];
            if let Some(end) = after_open.find("}}") {
                vars.push(after_open[..end].to_owned());
                rest = &after_open[end + 2..];
            } else {
                break;
            }
        }

        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_known_vars() {
        let mut vars = HashMap::new();
        vars.insert("NAME".to_owned(), "gaiad".to_owned());
        vars.insert("version".to_owned(), "v1.0".to_owned());

        let out = TemplateEngine::render("{{NAME}}-{{version}}", &vars);
        assert_eq!(out, "gaiad-v1.0");
    }

    #[test]
    fn render_preserves_unknown_vars() {
        let vars = HashMap::new();
        let out = TemplateEngine::render("{{UNKNOWN}}", &vars);
        assert_eq!(out, "{{UNKNOWN}}");
    }

    #[test]
    fn render_handles_no_placeholders() {
        let vars = HashMap::new();
        let out = TemplateEngine::render("plain text", &vars);
        assert_eq!(out, "plain text");
    }

    #[test]
    fn render_handles_adjacent_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("A".to_owned(), "1".to_owned());
        vars.insert("B".to_owned(), "2".to_owned());

        let out = TemplateEngine::render("{{A}}{{B}}", &vars);
        assert_eq!(out, "12");
    }

    #[test]
    fn unresolved_vars_finds_all() {
        let vars = TemplateEngine::unresolved_vars("{{A}} and {{B}}");
        assert_eq!(vars, vec!["A", "B"]);
    }

    #[test]
    fn unresolved_vars_empty_for_plain_text() {
        let vars = TemplateEngine::unresolved_vars("no vars");
        assert!(vars.is_empty());
    }

    #[test]
    fn render_preserves_multibyte_utf8() {
        let mut vars = HashMap::new();
        vars.insert("GREETING".to_owned(), "hola".to_owned());

        let out = TemplateEngine::render("\u{00e9}\u{00fc}\u{00f1} {{GREETING}} \u{1f600}!", &vars);
        assert_eq!(out, "\u{00e9}\u{00fc}\u{00f1} hola \u{1f600}!");
    }

    #[test]
    fn render_handles_unclosed_braces() {
        let vars = HashMap::new();
        let out = TemplateEngine::render("start {{OPEN", &vars);
        assert_eq!(out, "start {{OPEN");
    }
}
