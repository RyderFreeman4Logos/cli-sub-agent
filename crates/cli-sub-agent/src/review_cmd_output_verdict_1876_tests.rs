use super::*;
use csa_session::FindingsFile;

fn read_findings_toml(session_dir: &Path) -> FindingsFile {
    let findings_path = session_dir.join("output").join("findings.toml");
    toml::from_str(&fs::read_to_string(findings_path).expect("read findings.toml"))
        .expect("parse findings.toml")
}

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn write_empty_findings_toml(session_dir: &Path) {
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
}

#[test]
fn issue_1876_codex_pn_findings_populate_toml_counts_and_fail() {
    let session_id = "01TEST1876PNFINDINGS0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-pn-findings", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [P1][correctness] Dry-run GC/session clean now calls a mutating liveness probe ...
2. [P2][correctness/test-gap] Live orphan directories are still listed as removable ...

## Overall Risk

High
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
    )
    .expect("write suggestion.toml");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(findings.findings[1].severity, Severity::Medium);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_high_finding_populates_findings_toml_not_counts_only() {
    let session_id = "01TEST1876HIGHFINDING00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-high-finding", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [high][correctness] Dry-run cleanup still mutates session liveness state.

## Overall Risk

High
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_arbitrary_pn_finding_populates_low_findings_toml() {
    let session_id = "01TEST1876P7FINDING000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-p7-finding", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [P7][style] Reviewer emitted a priority tag outside the historical P0-P4 range.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::Low);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_unparsed_enumerated_findings_section_fails_closed() {
    let session_id = "01TEST1876UNPARSEDFIND";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-unparsed-findings", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [severity][correctness] Parser cannot classify this enumerated finding.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_resume_to_fix_suggestion_never_passes() {
    let session_id = "01TEST1876RESUMETOFIX00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-resume-to-fix", session_id);

    write_empty_findings_toml(&session_dir);
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
    )
    .expect("write suggestion.toml");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_nonzero_counts_with_empty_findings_fail_closed() {
    let session_id = "01TEST1876COUNTMISMATCH";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-count-mismatch", session_id);

    write_empty_findings_toml(&session_dir);
    let mut verdict =
        ReviewVerdictArtifact::from_parts(session_id, ReviewDecision::Pass, "CLEAN", &[], vec![]);
    verdict.severity_counts.insert(Severity::High, 1);

    enforce_final_verdict_consistency(&session_dir, &mut verdict).expect("enforce consistency");

    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("severity_counts_findings_mismatch")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_nonempty_findings_with_zero_counts_fail_closed() {
    let mut counts = BTreeMap::new();
    for severity in [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
    ] {
        counts.insert(severity, 0);
    }

    let decision = derive_decision_from_severity_counts(
        &counts,
        false,
        None,
        Some(ReviewDecision::Pass),
        || Ok(false),
        || Ok(false),
        || Ok(false),
    )
    .expect("derive decision");

    assert_eq!(decision, ReviewDecision::Fail);
}

#[test]
fn issue_1887_physical_details_word_severity_findings_fail_and_populate_counts() {
    let session_id = "01TEST1887PHYSDETAILSHIGH";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1887-physical-details-high", session_id);

    write_empty_findings_toml(&session_dir);
    fs::write(
        session_dir.join("output").join("details.md"),
        r#"## Findings

1. [High][regression] Shared monolith checker ignores non-Rust files (scripts/monolith/check.sh:280, confidence=0.94)
2. [Medium][test-gap] Monolith checker shell tests are not run by repo verification (scripts/tests/monolith-check-tests.sh:249, confidence=0.89)

## AGENTS.md Checklist

| Rule | Status |
| --- | --- |
| 016 testing | VIOLATION via finding MONO-002 |
"#,
    )
    .expect("write details.md");
    assert!(!session_dir.join("output").join("index.toml").exists());

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "scripts/monolith/check.sh"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 280);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1887_physical_details_empty_findings_stays_pass() {
    let session_id = "01TEST1887PHYSDETAILSPASS";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1887-physical-details-pass", session_id);

    write_empty_findings_toml(&session_dir);
    fs::write(
        session_dir.join("output").join("details.md"),
        "## Findings\n\nNo findings.\n",
    )
    .expect("write details.md");
    assert!(!session_dir.join("output").join("index.toml").exists());

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1887_high_category_numbered_line_parses_to_high() {
    let session_id = "01TEST1887PARSEHIGH000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1887-parse-high", session_id);

    fs::write(
        session_dir.join("output").join("details.md"),
        "## Findings\n\n1. [High][regression] Parser recognizes word severity (src/lib.rs:7, confidence=0.94)\n",
    )
    .expect("write details.md");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(findings.findings[0].file_ranges[0].path, "src/lib.rs");
    assert_eq!(findings.findings[0].file_ranges[0].start, 7);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1887_medium_category_numbered_line_parses_to_medium() {
    let session_id = "01TEST1887PARSEMEDIUM0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1887-parse-medium", session_id);

    fs::write(
        session_dir.join("output").join("details.md"),
        "## Findings\n\n1. [Medium][test-gap] Parser recognizes medium severity (tests/review.rs:42, confidence=0.89)\n",
    )
    .expect("write details.md");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::Medium);
    assert_eq!(findings.findings[0].file_ranges[0].path, "tests/review.rs");
    assert_eq!(findings.findings[0].file_ranges[0].start, 42);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1887_checklist_violation_referencing_finding_id_fails_closed() {
    let session_id = "01TEST1887CHECKLIST000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1887-checklist-violation", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

No findings.

## AGENTS.md Checklist

| Rule | Status |
| --- | --- |
| 016 testing | VIOLATION via finding MONO-001 |
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

const ISSUE_1896_DETAILS: &str = r#"# Code Review Report

## Scope
- Scope: `range:main...HEAD`
- Mode: `review-only`
- Review mode: `standard`
- Security mode: `auto`
- Project profile: `rust`

## Findings

1. [high][correctness] `PathEnvGuard` mutates global `PATH` in a parallel test binary (`crates/csa-process/src/tool_liveness_tests.rs:37`, confidence=0.86)

Trigger: `cargo test -p csa-process` runs tests in parallel by default. The new test changes process-wide `PATH`, while the same crate has many tests that concurrently spawn `sh`, `sleep`, `bash`, `echo`, and `true` through `Command::new(...)`, which reads `PATH`.

Expected: the test should avoid process-global env mutation, or run the probe in an isolated subprocess with `.env("PATH", patched_path)`, or otherwise prove no concurrent environment access can occur.

Actual: `PathEnvGuard::prepend` calls `unsafe { std::env::set_var("PATH", joined) }`, and `Drop` restores/removes `PATH`. The safety comment only says tests do not concurrently mutate `PATH`; Rust's safety requirement is stronger because concurrent environment reads are also unsafe on Unix.

Impact: test-only undefined behavior / CI instability under Rust 2024 env-mutation rules. This is not a design preference; it is an unsafe-block soundness issue.

Evidence:
- New unsafe env mutation: `crates/csa-process/src/tool_liveness_tests.rs:35-37`
- Restore path repeats the same global mutation: `crates/csa-process/src/tool_liveness_tests.rs:45-49`
- Same test binary contains concurrent PATH readers via command spawning, e.g. `tool_liveness_tests.rs:80`, `:115`, `:133`, plus many `lib_tests*.rs` command spawns found by `rg`.

Class sweep: 2 new same-class sites in this diff: initial `set_var("PATH", ...)` and Drop restore/remove.

## Cross-Dimension Blocking Enumeration
1. Correctness / unsafe soundness: global `PATH` mutation in a parallel test binary.
2. Security: no independent blocker found.
3. Contract/doc-sync: no independent blocker found.
4. Ordering/completeness: no independent blocker found.
"#;

#[test]
fn issue_1896_golden_progress_full_md_does_not_hide_details_findings() {
    let session_id = "01TEST1896GOLDENFULLMD00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-golden-full-md", session_id);

    write_empty_findings_toml(&session_dir);
    fs::write(
        session_dir
            .join("output")
            .join(crate::review_cmd::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER),
        "",
    )
    .expect("write synthetic marker");
    fs::write(
        session_dir.join("output").join("full.md"),
        "Loaded review protocol.\nCollecting diff context.\nPreparing final report.\n",
    )
    .expect("write progress-only full.md");
    fs::write(
        session_dir.join("output").join("summary.md"),
        "One high-severity issue found.\n",
    )
    .expect("write summary.md");
    fs::write(
        session_dir.join("output").join("details.md"),
        ISSUE_1896_DETAILS,
    )
    .expect("write details.md");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "crates/csa-process/src/tool_liveness_tests.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 37);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1896_ordinal_double_bracket_high_finding_parses() {
    let session_id = "01TEST1896PARSEHIGH000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-parse-high", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [high][correctness] Parser recognizes this finding (`crates/example/src/lib.rs:12`, confidence=0.90)
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "crates/example/src/lib.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 12);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1896_cross_dimension_concrete_blocker_fails_closed() {
    let session_id = "01TEST1896CROSSBLOCKER";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-cross-blocker", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

No findings.

## Cross-Dimension Blocking Enumeration
1. Correctness / unsafe soundness: global PATH mutation in a parallel test binary.
2. Security: no independent blocker found.
3. Contract/doc-sync: none.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1896_findings_none_and_empty_cross_dimension_stays_pass() {
    let session_id = "01TEST1896CLEANNONE000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1896-clean-none", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Findings: none.

## Cross-Dimension Blocking Enumeration
1. Correctness: no independent blocker found.
2. Security: none.
3. Contract/doc-sync: no independent blockers found.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
