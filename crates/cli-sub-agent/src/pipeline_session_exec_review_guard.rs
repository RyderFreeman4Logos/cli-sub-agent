//! Review-aware writer guard (#1842).
//!
//! Commit-producing `csa run --sa-mode false` writer sessions are made
//! *review-aware*: an always-on, size-gated guard block is injected into the
//! writer prompt so the writer self-reviews against the SAME dimensions the
//! independent reviewer will use, before it commits. This reduces write→review
//! rounds (the motivating cluster, PR #1844, took nine rounds; hand-adding this
//! framing converged round nine in one).
//!
//! The injected guard carries the four contract elements of #1842:
//! 1. **You-will-be-reviewed framing** — the diff WILL be adversarially reviewed
//!    by a separate, heterogeneous CSA reviewer and the merge gate fails on ANY
//!    blocking finding. The framing also states this does NOT replace the
//!    independent reviewer (a self-reviewing writer shares its own blind spots).
//! 2. **The per-dimension checklist** — extracted from the SINGLE source of truth
//!    shared with the reviewer ([`REVIEW_PROTOCOL_SRC`], the `csa-review`
//!    pattern's `review-protocol.md`), plus the project's `.csa/review-checklist.md`
//!    when present (the same file the reviewer injects).
//! 3. **A self-review-before-commit mandate** — walk each dimension against the
//!    diff and report the per-dimension result before committing.
//! 4. **An anti-gold-plating clause** — match the contract, do not invent scope.
//!
//! Design constraints (validated by prior adversarial debate, #1842):
//! - **A (predicate scope):** injection is gated by [`is_review_aware_writer_session`],
//!   a documented predicate distinct from the post-exec uncommitted-change detector.
//! - **B (cache isolation):** the guard is appended through the DYNAMIC prompt
//!   layer (`PromptAssembly::append_dynamic_block`), never the cached static
//!   block, so a per-project checklist cannot poison the cross-project cache.
//! - **C (bounded read):** the embedded dimensions are extracted and capped; the
//!   project checklist reuses [`discover_review_checklist`]'s 4000-char bound.
//! - **D (atomic write):** this guard performs NO sidecar/metadata write — it only
//!   returns prompt text — so no write-atomicity concern applies.
//! - **E (single dimension source):** `review-protocol.md` is the canonical source;
//!   the dimensions live between `CSA:REVIEW_DIMENSIONS` markers and are guarded by
//!   the drift test below so the writer guard and reviewer cannot diverge.

use std::path::Path;

use tracing::warn;

use crate::review_context::discover_review_checklist;
use crate::run_cmd::{is_writer_session, working_tree_changed_lines};

/// Canonical review protocol shipped with the `csa-review` pattern, embedded at
/// compile time so the writer guard and the reviewer draw the per-dimension
/// checklist from the SAME source file (#1842 contract item 2 / constraint E).
/// A wrong path is a compile error — the strongest possible drift guard.
const REVIEW_PROTOCOL_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../patterns/csa-review/skills/csa-review/references/review-protocol.md"
));

/// Markers delimiting the canonical dimensions block inside [`REVIEW_PROTOCOL_SRC`].
const DIMENSIONS_START: &str = "<!-- CSA:REVIEW_DIMENSIONS:START -->";
const DIMENSIONS_END: &str = "<!-- CSA:REVIEW_DIMENSIONS:END -->";

/// Upper bound on dimension text injected into the prompt (#1842 constraint C —
/// mirrors `review_context::REVIEW_CHECKLIST_MAX_CHARS`).
const MAX_INJECTED_CHARS: usize = 4000;

/// Resume-session working-tree diffs at/under this changed-line count are treated
/// as trivial and receive the brief guard instead of the full per-dimension
/// checklist (#1842 size-gating). Fresh (first-turn) sessions always get the full
/// guard because their eventual diff size is unknown and presumed substantial.
const TRIVIAL_DIFF_MAX_LINES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardKind {
    Full,
    Brief,
}

/// Predicate gating review-aware writer-guard injection (#1842 constraint A).
///
/// The guard targets *commit-producing writer* sessions: a `csa run` invoked
/// with `--sa-mode false`. It is deliberately suppressed for:
/// - SA-mode orchestrator sessions (`--sa-mode true`) — they delegate, never commit;
/// - `review` / `debate` / `recon` and any non-`run` task type — read-only or
///   analysis sessions with no diff to self-review.
///
/// This is a SEPARATE, named concept from [`is_writer_session`]'s post-exec
/// uncommitted-change *detection*: that function answers "did this session leave
/// a dirty tree?", whereas this answers "should the writer prompt carry the
/// review-aware guard?". They share the same `!sa_mode && run` shape today (a
/// `--sa-mode false` run IS the commit-producing-writer population), so this
/// delegates rather than duplicating the boolean — but it is documented and named
/// independently so the two can diverge later without coupling.
///
/// Documented caveat (not a bug): a read-only `csa run --sa-mode false` research
/// task is indistinguishable from a writer at prompt-assembly time — no
/// "will commit" signal exists pre-exec. Such a session receives the advisory
/// guard harmlessly; with no diff, the self-review is a no-op.
pub(crate) fn is_review_aware_writer_session(sa_mode: bool, task_type: Option<&str>) -> bool {
    is_writer_session(sa_mode, task_type)
}

/// Select guard verbosity from the session's turn and existing diff size.
fn select_guard_kind(is_first_turn: bool, changed_lines: usize) -> GuardKind {
    if is_first_turn || changed_lines > TRIVIAL_DIFF_MAX_LINES {
        GuardKind::Full
    } else {
        GuardKind::Brief
    }
}

/// Extract the canonical dimensions block from the embedded review protocol.
///
/// Returns `None` (fail-open) when the markers are absent or the block is empty;
/// the drift test guarantees they are present in committed source.
fn extract_dimensions(protocol_src: &str) -> Option<&str> {
    let start = protocol_src.find(DIMENSIONS_START)? + DIMENSIONS_START.len();
    let rel_end = protocol_src.get(start..)?.find(DIMENSIONS_END)?;
    let block = protocol_src.get(start..start + rel_end)?.trim();
    (!block.is_empty()).then_some(block)
}

/// Truncate `s` to at most `max_bytes`, snapping back to a UTF-8 char boundary.
/// `str::floor_char_boundary` is still nightly-only, hence the manual loop.
fn cap_chars(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// You-will-be-reviewed framing (#1842 contract item 1).
const REVIEWED_FRAMING: &str = "\
YOU WILL BE REVIEWED. The diff you produce WILL be adversarially reviewed by a \
separate, heterogeneous CSA reviewer (a different model family than you). The \
merge gate FAILS on ANY blocking (HIGH/CRITICAL) finding, and every blocking \
finding forces another write->review round. Self-reviewing now, against the SAME \
dimensions the reviewer uses, is the cheapest path to a one-round merge. This \
self-review REDUCES rounds but does NOT replace the independent reviewer: you \
share your own blind spots, so the heterogeneous reviewer remains mandatory and \
is never skipped or weakened.";

/// Self-review-before-commit mandate (#1842 contract item 3).
const SELF_REVIEW_MANDATE: &str = "\
SELF-REVIEW BEFORE COMMIT (mandatory): before you run `git commit`, walk EACH \
dimension above against your own diff and state the per-dimension result -- PASS, \
FIXED (naming what you changed), or N/A (one-line reason). Do not commit until \
every dimension is addressed; a dimension you cannot honestly mark PASS or N/A is \
a blocking finding you must fix first.";

/// Anti-gold-plating clause (#1842 contract item 4).
const ANTI_GOLD_PLATING: &str = "\
MATCH THE CONTRACT -- NO GOLD-PLATING: implement exactly what the task specifies. \
Do NOT invent requirements, abstractions, configuration, or scope beyond the \
stated contract. Unrequested scope (speculative generality, premature \
abstraction, defensive handling of impossible states) is itself a review finding.";

/// Brief guard for trivial resume diffs: keeps all four contract elements in
/// condensed form but omits the full per-dimension block (#1842 size-gating).
const BRIEF_GUARD: &str = "\
<csa-review-aware-writer-guard kind=\"brief\">
YOU WILL BE REVIEWED: this change WILL be checked by a separate, heterogeneous CSA \
reviewer and the merge gate fails on any blocking (HIGH/CRITICAL) finding. Even \
for a small diff, before you `git commit` do a quick self-review across \
correctness, error handling, tests, security, and AGENTS.md compliance, and MATCH \
THE CONTRACT -- NO GOLD-PLATING. The independent reviewer still runs; this does \
not replace it.
</csa-review-aware-writer-guard>";

/// Assemble the full guard block from the dimensions and optional project checklist.
fn render_full_guard(dimensions: &str, project_checklist: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("<csa-review-aware-writer-guard>\n");
    out.push_str(REVIEWED_FRAMING);
    out.push_str("\n\n<review-dimensions>\n");
    out.push_str(dimensions);
    out.push_str("\n</review-dimensions>\n");
    if let Some(checklist) = project_checklist {
        // Same `<review-checklist>` framing the reviewer injects (review_cmd_resolve.rs).
        out.push_str("\n<review-checklist>\n");
        out.push_str(checklist);
        out.push_str("\n</review-checklist>\n");
    }
    out.push('\n');
    out.push_str(SELF_REVIEW_MANDATE);
    out.push_str("\n\n");
    out.push_str(ANTI_GOLD_PLATING);
    out.push_str("\n</csa-review-aware-writer-guard>");
    out
}

/// Build the review-aware writer guard for a session, or `None` when the session
/// is not a commit-producing writer (suppressed for SA-mode/review/debate/recon).
///
/// Routed by the caller through `PromptAssembly::append_dynamic_block` so the
/// guard — including the per-project checklist — never enters the cached static
/// prompt (#1842 constraint B). Performs NO metadata write (#1842 constraint D).
//
// #1762 seam: measuring average review rounds per cluster before/after this guard
// is intentionally out of scope here and tracked separately in issue #1762.
pub(super) fn build_review_writer_guard(
    sa_mode: bool,
    task_type: Option<&str>,
    is_first_turn: bool,
    project_root: &Path,
) -> Option<String> {
    if !is_review_aware_writer_session(sa_mode, task_type) {
        return None;
    }

    // Only resume turns have a materialized diff to size-gate on; a fresh turn is
    // presumed substantial and skips the git probe entirely.
    let changed_lines = if is_first_turn {
        0
    } else {
        working_tree_changed_lines(project_root)
    };

    match select_guard_kind(is_first_turn, changed_lines) {
        GuardKind::Brief => Some(BRIEF_GUARD.to_string()),
        GuardKind::Full => {
            let Some(dimensions) = extract_dimensions(REVIEW_PROTOCOL_SRC) else {
                warn!(
                    "review-aware writer guard: dimensions markers missing from \
                     review-protocol.md; emitting brief guard"
                );
                return Some(BRIEF_GUARD.to_string());
            };
            let dimensions = cap_chars(dimensions, MAX_INJECTED_CHARS);
            // discover_review_checklist already bounds the read at 4000 chars (#1842 C).
            let project_checklist = discover_review_checklist(project_root);
            Some(render_full_guard(dimensions, project_checklist.as_deref()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::prompt_cache::{PromptAssembly, STATIC_END, STATIC_START};

    /// Dimension names that MUST stay in sync between the reviewer protocol and
    /// the writer guard. Changing the canonical list requires updating this array
    /// in the same commit (rule 027 spirit) — the drift test fails otherwise.
    const EXPECTED_DIMENSIONS: &[&str] = &[
        "Correctness & regressions",
        "Error handling",
        "Test coverage",
        "Security",
        "AGENTS.md compliance",
        "Spec / plan alignment",
        "Deslop",
        "Language-specific",
    ];

    #[test]
    fn predicate_injects_for_writer_and_suppresses_read_only_kinds() {
        // Inject case: commit-producing --sa-mode false run.
        assert!(is_review_aware_writer_session(false, Some("run")));
        // Suppress cases.
        assert!(!is_review_aware_writer_session(true, Some("run")));
        assert!(!is_review_aware_writer_session(false, Some("review")));
        assert!(!is_review_aware_writer_session(false, Some("debate")));
        assert!(!is_review_aware_writer_session(false, Some("recon")));
        assert!(!is_review_aware_writer_session(false, None));
    }

    #[test]
    fn dimensions_extract_from_embedded_protocol_without_drift() {
        assert!(
            REVIEW_PROTOCOL_SRC.contains(DIMENSIONS_START)
                && REVIEW_PROTOCOL_SRC.contains(DIMENSIONS_END),
            "review-protocol.md must carry the CSA:REVIEW_DIMENSIONS markers"
        );
        let dims = extract_dimensions(REVIEW_PROTOCOL_SRC)
            .expect("dimensions block must be extractable from review-protocol.md");
        assert!(!dims.is_empty());

        for name in EXPECTED_DIMENSIONS {
            assert!(
                dims.contains(name),
                "canonical dimension `{name}` missing from review-protocol.md \
                 (writer guard and reviewer would diverge)"
            );
        }

        // Bidirectional sync gate: adding or removing a dimension bullet without
        // updating EXPECTED_DIMENSIONS fails this test.
        let bullet_count = dims
            .lines()
            .filter(|line| line.trim_start().starts_with("- **"))
            .count();
        assert_eq!(
            bullet_count,
            EXPECTED_DIMENSIONS.len(),
            "dimension bullet count drifted from EXPECTED_DIMENSIONS"
        );
    }

    #[test]
    fn full_guard_contains_all_four_contract_elements() {
        let dims = extract_dimensions(REVIEW_PROTOCOL_SRC).unwrap();
        let guard = render_full_guard(dims, None);

        // 1. you-will-be-reviewed framing (+ does-not-replace-reviewer caveat).
        assert!(guard.contains("YOU WILL BE REVIEWED"));
        assert!(guard.contains("does NOT replace the independent reviewer"));
        // 2. per-dimension checklist.
        assert!(guard.contains("<review-dimensions>"));
        assert!(guard.contains("Correctness & regressions"));
        // 3. self-review-before-commit mandate.
        assert!(guard.contains("SELF-REVIEW BEFORE COMMIT"));
        // 4. anti-gold-plating clause.
        assert!(guard.contains("NO GOLD-PLATING"));
    }

    #[test]
    fn full_guard_embeds_project_checklist_when_present() {
        let dims = extract_dimensions(REVIEW_PROTOCOL_SRC).unwrap();
        let guard = render_full_guard(dims, Some("PROJECT-SPECIFIC-RULE"));
        assert!(guard.contains("<review-checklist>"));
        assert!(guard.contains("PROJECT-SPECIFIC-RULE"));
    }

    #[test]
    fn size_gate_full_for_fresh_or_substantial_brief_for_trivial_resume() {
        // Fresh session: full regardless of (zero) diff.
        assert_eq!(select_guard_kind(true, 0), GuardKind::Full);
        // Resume with no/trivial diff: brief.
        assert_eq!(select_guard_kind(false, 0), GuardKind::Brief);
        assert_eq!(
            select_guard_kind(false, TRIVIAL_DIFF_MAX_LINES),
            GuardKind::Brief
        );
        // Resume with substantial diff: full.
        assert_eq!(
            select_guard_kind(false, TRIVIAL_DIFF_MAX_LINES + 1),
            GuardKind::Full
        );

        // Content contrast: full carries the dimension block, brief does not, but
        // both keep the you-will-be-reviewed framing.
        let dims = extract_dimensions(REVIEW_PROTOCOL_SRC).unwrap();
        let full = render_full_guard(dims, None);
        assert!(full.contains("<review-dimensions>"));
        assert!(!BRIEF_GUARD.contains("<review-dimensions>"));
        assert!(BRIEF_GUARD.contains("YOU WILL BE REVIEWED"));
        assert!(BRIEF_GUARD.contains("NO GOLD-PLATING"));
    }

    /// #1842 constraint B: a per-project checklist injected through the dynamic
    /// layer must NOT bleed into the cached static prompt shared across projects.
    #[test]
    fn per_project_checklist_does_not_bleed_into_static_cache() {
        let dims = extract_dimensions(REVIEW_PROTOCOL_SRC).unwrap();
        let guard_a = render_full_guard(dims, Some("PROJECT-A-CHECKLIST"));
        let guard_b = render_full_guard(dims, Some("PROJECT-B-CHECKLIST"));

        let static_region = |guard: &str| -> String {
            let mut assembly = PromptAssembly::new("user task".to_string(), true);
            // Cacheable, project-independent static context.
            assembly.add_static_or_append_dynamic("SHARED-STATIC-CONTEXT");
            // Production injection path for the guard.
            assembly.append_dynamic_block(guard);
            let prompt = assembly.finish();

            let start = prompt.find(STATIC_START).expect("static start marker");
            let end = prompt.find(STATIC_END).expect("static end marker");
            // The per-project checklist must live in the dynamic tail, after the
            // cached static block.
            let checklist_marker = if guard.contains("PROJECT-A-CHECKLIST") {
                "PROJECT-A-CHECKLIST"
            } else {
                "PROJECT-B-CHECKLIST"
            };
            let checklist_pos = prompt.find(checklist_marker).expect("checklist present");
            assert!(
                checklist_pos > end,
                "per-project checklist leaked into the cached static block"
            );
            prompt[start..end].to_string()
        };

        // The cached static region must be byte-identical across projects: the
        // only varying input (the guard's per-project checklist) never touches it.
        assert_eq!(
            static_region(&guard_a),
            static_region(&guard_b),
            "static cache differs across projects — guard poisoned the cache"
        );
    }

    #[test]
    fn build_guard_suppressed_for_non_writer_sessions() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            build_review_writer_guard(true, Some("run"), true, temp.path()).is_none(),
            "sa-mode orchestrator must not receive the writer guard"
        );
        assert!(
            build_review_writer_guard(false, Some("review"), true, temp.path()).is_none(),
            "review session must not receive the writer guard"
        );
    }

    #[test]
    fn build_guard_full_for_fresh_writer_session() {
        let temp = tempfile::tempdir().unwrap();
        let guard = build_review_writer_guard(false, Some("run"), true, temp.path())
            .expect("fresh writer session must receive the full guard");
        assert!(guard.contains("YOU WILL BE REVIEWED"));
        assert!(guard.contains("<review-dimensions>"));
        assert!(guard.contains("SELF-REVIEW BEFORE COMMIT"));
        assert!(guard.contains("NO GOLD-PLATING"));
    }
}
