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
