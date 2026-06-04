pub(crate) const REVIEW_DESIGN_PREFERENCE_ANCHOR: &str = r#"## Design preferences vs correctness bugs

When reviewing diffs that touch design-level choices (preserve-vs-overwrite semantics, where to persist artifacts, API surface shapes, which lifecycle stages trigger which side-effects), treat the current choice as a DESIGN PREFERENCE if it has a coherent rationale, not as a correctness bug.

Issue FAIL verdicts ONLY for:
- Demonstrable data loss (e.g. unconditional delete of in-flight data, wrong write order that overwrites newer state)
- Crash paths (unwrap on untrusted data, missing error handling that panics)
- Contract violations (schema breakage, broken backward compatibility within a stable API)
- Obvious security issues (injection, credential leakage, path traversal)

Design preferences may have multiple valid answers. If the current choice works, is readable, and has a rationale documented in the commit message or the code itself, prefer PASS for that dimension. Surface design concerns as LOW-severity advisory notes, never as blocking FAIL findings.

Rationale: review ping-pong on design preferences wastes iterations and does NOT converge to better code quality. A reviewer that flip-flops on the same design point across rounds is a stronger signal of design residual than of real bugs.
"#;

pub(crate) const REVIEW_SAME_CLASS_SITE_SWEEP_ANCHOR: &str = r#"## Bounded same-class site sweep

When you identify a concrete defect, sweep the reviewed diff/range for sibling sites with the same defect pattern before finalizing. Check related call sites, artifacts, branches, sinks, and finalization paths.

Report confirmed siblings as sub-items of one finding labeled `class sweep: N sites`; inspect at most 12 candidates, list up to 8 confirmed sites, and note `class sweep truncated` if more remain.

For structured artifacts, use the label in the summary or evidence text only. Do not add schema fields or change severity, verdict, consensus, or pass/fail threshold semantics; each site still needs file+line evidence.
"#;

pub(crate) const REVIEW_CROSS_DIMENSION_ENUMERATION_ANCHOR: &str = r#"## Cross-dimension blocking enumeration

This diff is small/medium (at or below the #1645 large-diff threshold). Before you finalize a FAIL verdict, make ONE bounded breadth-first pass that enumerates the INDEPENDENT blocking (HIGH/CRITICAL/P1) findings currently latent ACROSS the standard dimensions (correctness, concurrency, security, contract/doc-sync, ordering, completeness). Report them TOGETHER as an enumerated list; do NOT stop at the first blocking finding. K independent defects of DIFFERENT bug-classes that are all simultaneously inspectable in this same diff should surface in ONE round, because each extra round costs a full fix -> re-review -> diff re-ingestion cycle.

Bounds (do NOT exhaustively enumerate): cap the list at the single highest-severity blocking finding PER dimension (the top blocker per dimension); fold sibling sites of one finding under the bounded same-class site sweep, not here. This targets only findings ALREADY latent in the reviewed diff -- defects genuinely INTRODUCED by a later fix legitimately surface in a later round, so this does NOT promise single-round convergence.

This is distinct from the bounded same-class site sweep: that sweep broadens ONE finding's bug-class across its sibling SITES (intra-dimension depth, #1797); this enumeration broadens across DIFFERENT dimensions / bug-classes (cross-dimension breadth, #1841). Apply BOTH -- sweep each confirmed blocker for siblings AND enumerate the top blocker of every other dimension.
"#;

use std::path::Path;

pub(crate) fn append_design_anchor(prompt: &mut String) {
    append_anchor_if_missing(
        prompt,
        "## Design preferences vs correctness bugs",
        REVIEW_DESIGN_PREFERENCE_ANCHOR,
    );
    append_anchor_if_missing(
        prompt,
        "## Bounded same-class site sweep",
        REVIEW_SAME_CLASS_SITE_SWEEP_ANCHOR,
    );
}

/// Append the cross-dimension breadth-first blocking-enumeration anchor (#1841)
/// when the diff-size gate selected cross-dimension mode.
///
/// `enabled` is the #1645-threshold gate decision computed by
/// `resolve_review_enumeration_mode` in the review enumeration-mode module: small/medium
/// diffs (`true`) receive the breadth-first enumeration guidance, while large diffs
/// (`false`) keep the existing chunked/escalation review path UNCHANGED so the
/// reviewer's attention is not diluted across dimensions on an oversized diff (the
/// regression #1645 guards against). Idempotent: a second call is a no-op.
pub(crate) fn append_cross_dimension_enumeration_anchor(prompt: &mut String, enabled: bool) {
    if !enabled {
        return;
    }
    append_anchor_if_missing(
        prompt,
        "## Cross-dimension blocking enumeration",
        REVIEW_CROSS_DIMENSION_ENUMERATION_ANCHOR,
    );
}

fn append_anchor_if_missing(prompt: &mut String, heading: &str, anchor: &str) {
    if prompt.contains(heading) {
        return;
    }
    prompt.push_str("\n\n");
    prompt.push_str(anchor);
}

pub(crate) fn resolve_current_branch_via_vcs(project_root: &Path) -> Option<String> {
    let backend = csa_session::vcs_backends::create_vcs_backend(project_root);
    backend.current_branch(project_root).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical review protocol embedded at compile time so this runtime anchor
    /// and the single-source protocol cannot describe cross-dimension enumeration
    /// differently (mirrors the #1842 dimensions drift guard).
    const REVIEW_PROTOCOL_SRC: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../patterns/csa-review/skills/csa-review/references/review-protocol.md"
    ));

    const CROSS_DIMENSION_HEADING: &str = "## Cross-dimension blocking enumeration";

    #[test]
    fn cross_dimension_anchor_appended_only_when_enabled() {
        let mut enabled = String::from("base prompt");
        append_cross_dimension_enumeration_anchor(&mut enabled, true);
        assert!(
            enabled.contains(CROSS_DIMENSION_HEADING),
            "small/medium diff must receive the cross-dimension enumeration anchor"
        );

        let mut disabled = String::from("base prompt");
        append_cross_dimension_enumeration_anchor(&mut disabled, false);
        assert!(
            !disabled.contains(CROSS_DIMENSION_HEADING),
            "large-diff path must stay unchanged (no enumeration anchor injected)"
        );
    }

    #[test]
    fn cross_dimension_anchor_is_idempotent() {
        let mut prompt = String::new();
        append_cross_dimension_enumeration_anchor(&mut prompt, true);
        append_cross_dimension_enumeration_anchor(&mut prompt, true);
        assert_eq!(
            prompt.matches(CROSS_DIMENSION_HEADING).count(),
            1,
            "anchor must not stack across repeated calls"
        );
    }

    #[test]
    fn cross_dimension_anchor_states_cap_caveat_and_sweep_distinction() {
        let anchor = REVIEW_CROSS_DIMENSION_ENUMERATION_ANCHOR;
        // Explicit per-dimension cap (bounded attention budget).
        assert!(anchor.contains("per dimension"));
        // Independently-latent caveat: must not over-promise single-round convergence.
        assert!(anchor.contains("does NOT promise single-round convergence"));
        // #1797 (intra-dimension same-class) vs #1841 (cross-dimension breadth).
        assert!(anchor.contains("#1797"));
        assert!(anchor.contains("#1841"));
    }

    /// Protocol single-source presence/drift guard: the cross-dimension enumeration
    /// instruction AND the #1797-vs-#1841 distinction must live in
    /// review-protocol.md (the file the reviewer reads from disk) so the protocol
    /// and this runtime anchor cannot silently diverge.
    #[test]
    fn protocol_single_source_carries_cross_dimension_enumeration() {
        assert!(
            REVIEW_PROTOCOL_SRC.contains("Cross-dimension blocking enumeration"),
            "review-protocol.md must document cross-dimension blocking enumeration (#1841)"
        );
        assert!(
            REVIEW_PROTOCOL_SRC.contains("#1797") && REVIEW_PROTOCOL_SRC.contains("#1841"),
            "review-protocol.md must document the #1797 (same-class) vs #1841 (cross-dimension) distinction"
        );
    }
}
