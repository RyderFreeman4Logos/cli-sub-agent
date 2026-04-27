//! Shared thinking-budget vocabulary for model spec validation.

/// Named thinking-budget values accepted in `tool/provider/model/budget` specs.
pub const VALID_BUDGETS: &[&str] = &[
    "default",
    "low",
    "medium",
    "med",
    "high",
    "xhigh",
    "extra-high",
    "max",
];

/// Human-readable accepted thinking-budget values for validation errors.
pub const VALID_BUDGET_DESCRIPTION: &str =
    "default, low, medium, med, high, xhigh, extra-high, max, or a number";

/// Return whether `value` is an accepted thinking-budget keyword or custom
/// numeric token budget.
pub fn is_valid_budget(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    VALID_BUDGETS.contains(&normalized.as_str()) || normalized.parse::<u32>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_named_budget_case_insensitively() {
        assert!(is_valid_budget("XHIGH"));
    }

    #[test]
    fn accepts_numeric_budget() {
        assert!(is_valid_budget("5000"));
    }

    #[test]
    fn rejects_unknown_budget() {
        assert!(!is_valid_budget("minimal"));
    }
}
