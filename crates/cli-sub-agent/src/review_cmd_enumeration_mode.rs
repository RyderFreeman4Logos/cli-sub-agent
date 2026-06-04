//! Cross-dimension breadth-first blocking-enumeration mode (#1841).
//!
//! Derives, from the existing #1645 large-diff threshold machinery in the parent
//! diff-size module, whether a review should attempt a breadth-first pass
//! enumerating the top blocking finding across all standard dimensions in one
//! round, and appends the corresponding prompt anchor when so. Kept in its own
//! file so the parent #1645 diff-size module stays within the per-module token
//! budget.

use csa_session::ReviewDiffSize;

use super::LargeDiffWarning;

/// Cross-dimension breadth-first enumeration mode (#1841).
///
/// Selected from the existing #1645 large-diff threshold so the reviewer only
/// attempts a breadth-first pass over all dimensions when it can attend to the
/// whole diff at once. Large (or unmeasurable) diffs keep the pre-existing
/// chunked/escalation path, avoiding the attention-dilution regression #1645
/// guards against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewEnumerationMode {
    /// Small/medium diff (at or below the #1645 threshold): enumerate the top
    /// blocking finding per standard dimension in one pass before finalizing FAIL.
    CrossDimension,
    /// Large diff (above the #1645 threshold), or a diff that could not be sized:
    /// keep the existing chunked/escalation path; do NOT inject the breadth-first
    /// enumeration anchor.
    LargeDiffChunked,
}

impl ReviewEnumerationMode {
    /// Whether this mode injects the cross-dimension breadth-first enumeration
    /// anchor into the review prompt.
    fn enumerates_cross_dimension(self) -> bool {
        matches!(self, ReviewEnumerationMode::CrossDimension)
    }
}

/// Resolve the cross-dimension enumeration mode from the already-computed #1645
/// diff-size report, reusing the existing large-diff threshold machinery
/// ([`super::large_diff_warning`]) rather than introducing a second threshold:
///
/// - measured and NOT large -> [`ReviewEnumerationMode::CrossDimension`]
/// - measured and large, OR unmeasurable -> [`ReviewEnumerationMode::LargeDiffChunked`]
///
/// An unmeasurable diff (`diff_size == None`, e.g. the git diff failed)
/// conservatively keeps the existing path, so a diff that could not be sized never
/// forces breadth-first enumeration onto a potentially oversized change.
fn resolve_review_enumeration_mode(
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<LargeDiffWarning>,
) -> ReviewEnumerationMode {
    match diff_size {
        Some(_) if large_diff_warning.is_none() => ReviewEnumerationMode::CrossDimension,
        _ => ReviewEnumerationMode::LargeDiffChunked,
    }
}

/// Append the #1841 cross-dimension breadth-first blocking-enumeration anchor to
/// the review `prompt` when the #1645 diff-size gate selects cross-dimension mode
/// (small/medium diffs); no-op for large or unmeasurable diffs. Idempotent: a
/// repeated call does not stack the anchor.
pub(crate) fn append_cross_dimension_anchor(
    prompt: &mut String,
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<LargeDiffWarning>,
) {
    let mode = resolve_review_enumeration_mode(diff_size, large_diff_warning);
    crate::review_design_anchor::append_cross_dimension_enumeration_anchor(
        prompt,
        mode.enumerates_cross_dimension(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::large_diff_warning;
    use super::*;

    #[test]
    fn enumeration_mode_cross_dimension_for_measured_small_diff() {
        let size = ReviewDiffSize {
            files: 1,
            changed_lines: 40,
            bytes: 512,
            notes: Vec::new(),
        };
        let warning = large_diff_warning(&size, Some(1000));
        assert!(
            warning.is_none(),
            "40 lines is below the 1000-line threshold"
        );

        let mode = resolve_review_enumeration_mode(Some(&size), warning);
        assert_eq!(mode, ReviewEnumerationMode::CrossDimension);
        assert!(mode.enumerates_cross_dimension());
    }

    #[test]
    fn enumeration_mode_large_chunked_for_measured_large_diff() {
        let size = ReviewDiffSize {
            files: 9,
            changed_lines: 1500,
            bytes: 65536,
            notes: Vec::new(),
        };
        let warning = large_diff_warning(&size, Some(1000));
        assert!(
            warning.is_some(),
            "1500 lines is above the 1000-line threshold"
        );

        let mode = resolve_review_enumeration_mode(Some(&size), warning);
        assert_eq!(mode, ReviewEnumerationMode::LargeDiffChunked);
        assert!(!mode.enumerates_cross_dimension());
    }

    #[test]
    fn enumeration_mode_chunked_when_diff_size_unmeasurable() {
        // A diff that could not be sized must keep the existing path, not force
        // breadth-first enumeration onto a potentially oversized change.
        assert_eq!(
            resolve_review_enumeration_mode(None, None),
            ReviewEnumerationMode::LargeDiffChunked
        );
    }

    #[test]
    fn append_cross_dimension_anchor_gates_on_diff_size() {
        let small = ReviewDiffSize {
            files: 1,
            changed_lines: 40,
            bytes: 512,
            notes: Vec::new(),
        };
        let mut enabled = String::from("base prompt");
        append_cross_dimension_anchor(&mut enabled, Some(&small), None);
        assert!(
            enabled.contains("## Cross-dimension blocking enumeration"),
            "small/medium diff must receive the enumeration anchor"
        );

        let large = ReviewDiffSize {
            files: 9,
            changed_lines: 1500,
            bytes: 65536,
            notes: Vec::new(),
        };
        let warning = large_diff_warning(&large, Some(1000));
        let mut disabled = String::from("base prompt");
        append_cross_dimension_anchor(&mut disabled, Some(&large), warning);
        assert!(
            !disabled.contains("## Cross-dimension blocking enumeration"),
            "large diff must keep the existing path (no enumeration anchor)"
        );
    }
}
