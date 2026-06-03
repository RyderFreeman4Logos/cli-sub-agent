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
