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
/// steps cleanly.  Malformed expressions (unbalanced parens, empty) also
/// evaluate to false (fail-closed).
pub(crate) fn evaluate_condition(condition: &str, vars: &HashMap<String, String>) -> bool {
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Split on top-level " && " (parenthesis-depth 0).  Handles 2+ conjuncts
    // including negated and nested sub-expressions.
    if let Some(parts) = split_top_level_and(trimmed) {
        return parts.iter().all(|p| evaluate_condition(p, vars));
    }

    // Handle negation: !(expr)
    if let Some(inner) = trimmed.strip_prefix("!(").and_then(|s| s.strip_suffix(')')) {
        return !evaluate_condition(inner, vars);
    }

    // Handle parenthesized expression: (expr) — strip only when the outer
    // parens are the matching pair that wraps the entire expression.
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        if let Some(inner) = strip_balanced_parens(trimmed) {
            return evaluate_condition(inner, vars);
        }
    }

    // Base case: ${VAR} — substitute and check truthiness.
    // NOTE: bare variable names (e.g. `has_tests` without `${}`) are NOT
    // supported.  The weave compiler always emits `${VAR}` form.
    let resolved = substitute_vars(trimmed, vars);

    // If the resolved string still contains ${...}, the var was not provided → false
    if resolved.contains("${") {
        return false;
    }

    // Fail-closed: reject resolved strings that still look like malformed
    // expressions — leftover operators or unbalanced parentheses.
    if looks_malformed(&resolved) {
        return false;
    }

    // Truthy: non-empty and not literally "false" or "0"
    let lower = resolved.trim().to_lowercase();
    !lower.is_empty() && lower != "false" && lower != "0"
}

/// Split `expr` into parts at every ` && ` that occurs at parenthesis depth 0.
///
/// Returns `None` when there is no top-level ` && ` (i.e. the expression is
/// not a conjunction at the outermost level).
fn split_top_level_and(expr: &str) -> Option<Vec<&str>> {
    let bytes = expr.as_bytes();
    let mut depth: i32 = 0;
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0;
    let and_token = b" && ";
    let and_len = and_token.len();

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b' ' if depth == 0
                && i + and_len <= bytes.len()
                && &bytes[i..i + and_len] == and_token =>
            {
                parts.push(&expr[start..i]);
                i += and_len;
                start = i;
                continue;
            }
            _ => {}
        }
        if depth < 0 {
            // Unbalanced — fail-closed
            return None;
        }
        i += 1;
    }

    if parts.is_empty() {
        return None;
    }

    // Fail-closed: unbalanced parentheses across the whole expression
    if depth != 0 {
        return None;
    }

    parts.push(&expr[start..]);
    Some(parts)
}

/// Strip a single layer of balanced outer parentheses.
///
/// Returns `None` when the opening `(` does not match the final `)` (e.g. the
/// string contains `) && (` at depth 0, which means the "outer" parens
/// actually belong to separate sub-expressions).
fn strip_balanced_parens(expr: &str) -> Option<&str> {
    debug_assert!(expr.starts_with('(') && expr.ends_with(')'));
    let bytes = expr.as_bytes();
    let mut depth: i32 = 0;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                // If depth drops to 0 before the last char, the opening `(`
                // closed mid-string — the outer parens are not a matching pair.
                if depth == 0 && idx < bytes.len() - 1 {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(&expr[1..expr.len() - 1])
}

/// Return `true` when the resolved string looks like a malformed expression
/// rather than a simple value — unbalanced parentheses or leftover operators.
fn looks_malformed(s: &str) -> bool {
    let trimmed = s.trim();
    // Leftover conjunction/disjunction operators
    if trimmed.contains(" && ") || trimmed.contains(" || ") {
        return true;
    }
    // Trailing or leading operator fragments
    if trimmed.starts_with("&& ")
        || trimmed.starts_with("|| ")
        || trimmed.ends_with(" &&")
        || trimmed.ends_with(" ||")
    {
        return true;
    }
    // Unbalanced parentheses
    let mut depth: i32 = 0;
    for b in trimmed.bytes() {
        match b {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return true;
        }
    }
    depth != 0
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

    #[test]
    fn three_conjuncts_with_negation() {
        // P1 regression: 3+ conjuncts broke the old `find(") && (")` logic.
        let expr = "(!(${COMMENT_IS_FALSE_POSITIVE})) && (${REVIEW_HAS_ISSUES}) && (!(${COMMENT_IS_STALE}))";

        // All conditions met: !(unset=false)=true && yes=true && !(unset=false)=true → true
        let mut vars = HashMap::new();
        vars.insert("REVIEW_HAS_ISSUES".into(), "yes".into());
        assert!(evaluate_condition(expr, &vars));

        // Middle var unset → false
        let empty = HashMap::new();
        assert!(!evaluate_condition(expr, &empty));

        // Negated var is truthy → !(true)=false → whole conjunction false
        let mut fp_set = HashMap::new();
        fp_set.insert("REVIEW_HAS_ISSUES".into(), "yes".into());
        fp_set.insert("COMMENT_IS_FALSE_POSITIVE".into(), "yes".into());
        assert!(!evaluate_condition(expr, &fp_set));
    }

    #[test]
    fn nested_conjunction() {
        // Nested: (!(A)) && ((B) && (!(C)))
        let expr = "(!(${A})) && ((${B}) && (!(${C})))";

        let mut vars = HashMap::new();
        vars.insert("B".into(), "yes".into());
        // A unset → !(false)=true, B=true, C unset → !(false)=true → true
        assert!(evaluate_condition(expr, &vars));

        // C set → !(true)=false → inner conjunction false → whole false
        let mut vars2 = HashMap::new();
        vars2.insert("B".into(), "yes".into());
        vars2.insert("C".into(), "yes".into());
        assert!(!evaluate_condition(expr, &vars2));
    }

    #[test]
    fn malformed_expression_is_false() {
        let vars = HashMap::new();
        // Empty expression
        assert!(!evaluate_condition("", &vars));
        // Unbalanced parens — inner var unset, so base-case resolves to false
        assert!(!evaluate_condition("((${A})", &vars));
        // Unresolved variable reference → false
        assert!(!evaluate_condition("${DOES_NOT_EXIST}", &vars));
    }

    #[test]
    fn unbalanced_parens_with_set_vars_is_false() {
        // P1-1: Unbalanced parens must fail-closed even when variables resolve
        let mut vars = HashMap::new();
        vars.insert("A".into(), "yes".into());
        // Extra opening paren
        assert!(!evaluate_condition("((${A})", &vars));
        // Extra closing paren
        assert!(!evaluate_condition("(${A}))", &vars));
        // Unbalanced in conjunction
        assert!(!evaluate_condition("((${A}) && (${A})", &vars));
    }

    #[test]
    fn trailing_operator_is_false() {
        // P1-1: Trailing && should fail-closed
        let mut vars = HashMap::new();
        vars.insert("A".into(), "yes".into());
        assert!(!evaluate_condition("(${A}) && ", &vars));
        assert!(!evaluate_condition(" && (${A})", &vars));
    }

    #[test]
    fn malformed_with_set_vars_fails_closed() {
        // P1-2: Verify fail-closed exercises with *set* variables, not just unset
        let mut vars = HashMap::new();
        vars.insert("A".into(), "yes".into());
        vars.insert("B".into(), "true".into());
        // Leftover operator after substitution
        assert!(!evaluate_condition("(${A}) && (${B}) && ", &vars));
        // Unbalanced opening paren with conjunction
        assert!(!evaluate_condition("((${A}) && (${B})", &vars));
    }

    #[test]
    fn empty_condition_is_false() {
        let vars = HashMap::new();
        assert!(!evaluate_condition("", &vars));
        assert!(!evaluate_condition("   ", &vars));
    }
}
