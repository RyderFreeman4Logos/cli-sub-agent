//! Condition evaluation for plan step execution.
//!
//! Supports simple boolean expressions used in workflow IF/FOR conditions:
//! - `${VAR}` → true when var is non-empty (and not "false"/"0")
//! - `!(expr)` → logical NOT
//! - `(a) && (b)` → logical AND

use std::collections::HashMap;

/// Evaluate a condition expression against the current variables.
///
/// Unresolved `${VAR}` references (where the var was not provided) evaluate to
/// false, allowing workflows with optional condition variables to skip those
/// steps cleanly.
pub(crate) fn evaluate_condition(condition: &str, vars: &HashMap<String, String>) -> bool {
    let trimmed = condition.trim();

    // Handle conjunction: (a) && (b)
    // Must check before parenthesized-expression stripping to avoid
    // incorrectly treating "(a) && (b)" as a single parenthesized expr.
    if let Some(pos) = trimmed.find(") && (") {
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            let left = &trimmed[1..pos];
            // ") && (" is 6 chars; skip to the content after the opening '('
            let right = &trimmed[pos + 6..trimmed.len() - 1];
            return evaluate_condition(left, vars) && evaluate_condition(right, vars);
        }
    }

    // Handle negation: !(expr)
    if let Some(inner) = trimmed.strip_prefix("!(").and_then(|s| s.strip_suffix(')')) {
        return !evaluate_condition(inner, vars);
    }

    // Handle simple parenthesized expression: (expr)
    // Only strip if the parens are balanced (no inner conjunction).
    if trimmed.starts_with('(') && trimmed.ends_with(')') && !trimmed.contains(" && ") {
        return evaluate_condition(&trimmed[1..trimmed.len() - 1], vars);
    }

    // Base case: ${VAR} — substitute and check truthiness
    let resolved = substitute_vars(trimmed, vars);

    // If the resolved string still contains ${...}, the var was not provided → false
    if resolved.contains("${") {
        return false;
    }

    // Truthy: non-empty and not literally "false" or "0"
    let lower = resolved.trim().to_lowercase();
    !lower.is_empty() && lower != "false" && lower != "0"
}

/// Substitute `${VAR}` placeholders in a string.
fn substitute_vars(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("${{{}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_var_is_false() {
        let vars = HashMap::new();
        assert!(!evaluate_condition("${UNSET}", &vars));
    }

    #[test]
    fn empty_var_is_false() {
        let mut vars = HashMap::new();
        vars.insert("EMPTY".into(), "".into());
        assert!(!evaluate_condition("${EMPTY}", &vars));
    }

    #[test]
    fn false_literal_is_false() {
        let mut vars = HashMap::new();
        vars.insert("FLAG".into(), "false".into());
        assert!(!evaluate_condition("${FLAG}", &vars));
    }

    #[test]
    fn zero_is_false() {
        let mut vars = HashMap::new();
        vars.insert("FLAG".into(), "0".into());
        assert!(!evaluate_condition("${FLAG}", &vars));
    }

    #[test]
    fn nonempty_var_is_true() {
        let mut vars = HashMap::new();
        vars.insert("FLAG".into(), "yes".into());
        assert!(evaluate_condition("${FLAG}", &vars));
    }

    #[test]
    fn negation() {
        let mut vars = HashMap::new();
        vars.insert("FLAG".into(), "yes".into());
        assert!(!evaluate_condition("!(${FLAG})", &vars));

        let empty_vars = HashMap::new();
        assert!(evaluate_condition("!(${FLAG})", &empty_vars));
    }

    #[test]
    fn conjunction() {
        let mut vars = HashMap::new();
        vars.insert("A".into(), "yes".into());
        vars.insert("B".into(), "yes".into());
        assert!(evaluate_condition("(${A}) && (${B})", &vars));

        let mut partial = HashMap::new();
        partial.insert("A".into(), "yes".into());
        assert!(!evaluate_condition("(${A}) && (${B})", &partial));
    }

    #[test]
    fn nested_not_and_and() {
        // Pattern from dev-to-merge: (${BOT_HAS_ISSUES}) && (!(${COMMENT_IS_FALSE_POSITIVE}))
        let mut vars = HashMap::new();
        vars.insert("BOT_HAS_ISSUES".into(), "yes".into());
        // COMMENT_IS_FALSE_POSITIVE not set → !(false) = true
        assert!(evaluate_condition(
            "(${BOT_HAS_ISSUES}) && (!(${COMMENT_IS_FALSE_POSITIVE}))",
            &vars
        ));

        // Both unset → false && true = false
        let empty = HashMap::new();
        assert!(!evaluate_condition(
            "(${BOT_HAS_ISSUES}) && (!(${COMMENT_IS_FALSE_POSITIVE}))",
            &empty
        ));
    }
}
