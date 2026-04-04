use super::*;
use tempfile::tempdir;

// ── extract_findings_from_result ────────────────────────────────────────────

#[test]
fn test_extract_numbered_findings() {
    let output = "\
Review Summary
1. Missing error context in anyhow chains throughout module
2. Unchecked unwrap in test helper functions
3. Public API lacks documentation for error variants
";
    let findings = extract_findings_from_result(output);
    assert_eq!(findings.len(), 3);
    assert_eq!(
        findings[0],
        "Missing error context in anyhow chains throughout module"
    );
    assert_eq!(findings[1], "Unchecked unwrap in test helper functions");
    assert_eq!(
        findings[2],
        "Public API lacks documentation for error variants"
    );
}

#[test]
fn test_extract_bullet_findings() {
    let output = "\
Issues found:
- Missing error context in anyhow chains throughout module
- Unchecked unwrap in test helper functions
* Public API lacks documentation for error variants
";
    let findings = extract_findings_from_result(output);
    assert_eq!(findings.len(), 3);
}

#[test]
fn test_extract_bracketed_findings() {
    let output = "\
[R01] Missing error context in anyhow chains throughout module
[HIGH] Unchecked unwrap in production code path
[R03] Resource cleanup missing on error path in sandbox module
";
    let findings = extract_findings_from_result(output);
    assert_eq!(findings.len(), 3);
    assert!(findings[0].contains("Missing error context"));
}

#[test]
fn test_extract_skips_short_lines() {
    let output = "\
1. OK
- Fine
- This is a real finding that should be extracted properly
";
    let findings = extract_findings_from_result(output);
    // "OK" and "Fine" are < 10 chars, should be skipped
    assert_eq!(findings.len(), 1);
}

#[test]
fn test_extract_truncates_long_findings() {
    let long_text = "a".repeat(200);
    let output = format!("1. {long_text}");
    let findings = extract_findings_from_result(&output);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].len() <= 123); // 120 + "..."
    assert!(findings[0].ends_with("..."));
}

#[test]
fn test_extract_handles_empty_output() {
    let findings = extract_findings_from_result("");
    assert!(findings.is_empty());
}

#[test]
fn test_extract_ignores_plain_text() {
    let output = "This review looks good overall. No major issues found.";
    let findings = extract_findings_from_result(output);
    assert!(findings.is_empty());
}

#[test]
fn test_extract_numbered_with_parens() {
    let output = "1) Missing error context in anyhow chains throughout module\n";
    let findings = extract_findings_from_result(output);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].contains("Missing error context"));
}

// ── dedupe_against_checklist ────────────────────────────────────────────────

#[test]
fn test_dedupe_removes_matching_findings() {
    let temp = tempdir().unwrap();
    let checklist = temp.path().join("checklist.md");
    std::fs::write(
        &checklist,
        "# Checklist\n- [ ] RAII guards call finalize before process exit\n",
    )
    .unwrap();

    let findings = vec![
        "RAII guards must call finalize before calling process exit".to_string(),
        "Missing timeout handling in subprocess lifecycle".to_string(),
    ];

    let result = dedupe_against_checklist(&findings, &checklist);

    // First finding overlaps with checklist item, second doesn't
    assert_eq!(result.len(), 1);
    assert!(result[0].contains("timeout"));
}

#[test]
fn test_dedupe_keeps_all_when_no_checklist() {
    let temp = tempdir().unwrap();
    let nonexistent = temp.path().join("no-such-file.md");

    let findings = vec![
        "Finding one about error handling".to_string(),
        "Finding two about resource cleanup".to_string(),
    ];

    let result = dedupe_against_checklist(&findings, &nonexistent);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_dedupe_keeps_all_when_empty_checklist() {
    let temp = tempdir().unwrap();
    let checklist = temp.path().join("checklist.md");
    std::fs::write(&checklist, "# Empty Checklist\n").unwrap();

    let findings = vec!["Some finding about missing error handling".to_string()];

    let result = dedupe_against_checklist(&findings, &checklist);
    assert_eq!(result.len(), 1);
}

// ── append_candidates ───────────────────────────────────────────────────────

#[test]
fn test_append_creates_new_candidates_file() {
    let temp = tempdir().unwrap();
    let candidates_path = temp.path().join(".csa").join("candidates.md");

    let findings = vec![
        "Missing error context in anyhow chains throughout module".to_string(),
        "Unchecked unwrap in production code path".to_string(),
    ];

    append_candidates(&findings, &candidates_path).unwrap();

    let content = std::fs::read_to_string(&candidates_path).unwrap();
    assert!(content.contains("[count:1] Missing error context"));
    assert!(content.contains("[count:1] Unchecked unwrap"));
}

#[test]
fn test_append_increments_existing_candidate() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    let candidates_path = csa_dir.join("candidates.md");

    // Pre-populate with one candidate
    std::fs::write(
        &candidates_path,
        "# Review Findings Candidates\n- [count:2] Missing error context in anyhow chains\n",
    )
    .unwrap();

    // Append a similar finding (fuzzy match should increment)
    let findings = vec!["Missing error context in anyhow chain propagation".to_string()];
    append_candidates(&findings, &candidates_path).unwrap();

    let content = std::fs::read_to_string(&candidates_path).unwrap();
    assert!(content.contains("[count:3]"));
    // Should NOT have a new [count:1] entry
    assert!(!content.contains("[count:1]"));
}

#[test]
fn test_append_adds_new_when_no_match() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();
    let candidates_path = csa_dir.join("candidates.md");

    std::fs::write(
        &candidates_path,
        "# Review Findings Candidates\n- [count:1] Missing error context in anyhow chains\n",
    )
    .unwrap();

    let findings = vec!["Subprocess lifecycle missing RAII cleanup guard".to_string()];
    append_candidates(&findings, &candidates_path).unwrap();

    let content = std::fs::read_to_string(&candidates_path).unwrap();
    // Original preserved
    assert!(content.contains("[count:1] Missing error context"));
    // New one added
    assert!(content.contains("[count:1] Subprocess lifecycle"));
}

#[test]
fn test_append_empty_findings_noop() {
    let temp = tempdir().unwrap();
    let candidates_path = temp.path().join("candidates.md");

    append_candidates(&[], &candidates_path).unwrap();

    assert!(!candidates_path.exists());
}

// ── promote_candidates ──────────────────────────────────────────────────────

#[test]
fn test_promote_moves_above_threshold() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    let candidates_path = csa_dir.join("candidates.md");
    let checklist_path = csa_dir.join("review-checklist.md");

    std::fs::write(
        &candidates_path,
        "# Review Findings Candidates\n\
         - [count:3] Missing error context in anyhow chains\n\
         - [count:1] Unchecked unwrap in test helpers\n",
    )
    .unwrap();
    std::fs::write(
        &checklist_path,
        "# Project Review Checklist\n\n- [ ] Existing item\n",
    )
    .unwrap();

    let promoted = promote_candidates(&candidates_path, &checklist_path, Some(3)).unwrap();

    assert_eq!(promoted.len(), 1);
    assert!(promoted[0].contains("Missing error context"));

    // Verify checklist was updated
    let checklist = std::fs::read_to_string(&checklist_path).unwrap();
    assert!(checklist.contains("- [ ] Missing error context"));
    assert!(checklist.contains("- [ ] Existing item"));

    // Verify candidates file was updated (promoted item removed)
    let candidates = std::fs::read_to_string(&candidates_path).unwrap();
    assert!(!candidates.contains("Missing error context"));
    assert!(candidates.contains("[count:1] Unchecked unwrap"));
}

#[test]
fn test_promote_nothing_below_threshold() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    let candidates_path = csa_dir.join("candidates.md");
    let checklist_path = csa_dir.join("review-checklist.md");

    std::fs::write(
        &candidates_path,
        "# Review Findings Candidates\n- [count:2] Some finding below threshold\n",
    )
    .unwrap();
    std::fs::write(&checklist_path, "# Checklist\n").unwrap();

    let promoted = promote_candidates(&candidates_path, &checklist_path, Some(3)).unwrap();

    assert!(promoted.is_empty());

    // Checklist unchanged
    let checklist = std::fs::read_to_string(&checklist_path).unwrap();
    assert!(!checklist.contains("Some finding"));
}

#[test]
fn test_promote_creates_checklist_if_missing() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    let candidates_path = csa_dir.join("candidates.md");
    let checklist_path = csa_dir.join("review-checklist.md");

    std::fs::write(
        &candidates_path,
        "# Review Findings Candidates\n- [count:3] New promoted finding for checklist\n",
    )
    .unwrap();

    // No checklist file exists
    let promoted = promote_candidates(&candidates_path, &checklist_path, Some(3)).unwrap();

    assert_eq!(promoted.len(), 1);
    assert!(checklist_path.exists());

    let checklist = std::fs::read_to_string(&checklist_path).unwrap();
    assert!(checklist.contains("- [ ] New promoted finding"));
}

// ── accumulate_findings (integration) ───────────────────────────────────────

#[test]
fn test_accumulate_findings_end_to_end() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    // Create an existing checklist
    std::fs::write(
        csa_dir.join("review-checklist.md"),
        "# Checklist\n- [ ] Existing review item about error handling\n",
    )
    .unwrap();

    let review_output = "\
Review found the following issues:
1. Missing error context in anyhow chains throughout module
2. Subprocess lifecycle missing RAII cleanup guard pattern
3. Config struct with serde default lacks is_default method
";

    accumulate_findings(temp.path(), review_output);

    // Candidates file should exist with new findings (not the one matching checklist)
    let candidates = std::fs::read_to_string(csa_dir.join("review-findings-candidates.md"));
    assert!(candidates.is_ok());
    let content = candidates.unwrap();
    assert!(content.contains("[count:1]"));
}

#[test]
fn test_accumulate_findings_no_findings() {
    let temp = tempdir().unwrap();
    let csa_dir = temp.path().join(".csa");
    std::fs::create_dir_all(&csa_dir).unwrap();

    accumulate_findings(temp.path(), "Everything looks good!");

    // No candidates file should be created
    assert!(!csa_dir.join("review-findings-candidates.md").exists());
}

// ── keyword helpers ─────────────────────────────────────────────────────────

#[test]
fn test_keywords_extracts_significant_words() {
    let kw = keywords("Missing error context in anyhow chains");
    assert!(kw.contains("missing"));
    assert!(kw.contains("error"));
    assert!(kw.contains("context"));
    assert!(kw.contains("anyhow"));
    assert!(kw.contains("chains"));
    // "in" is < 3 chars, should be excluded
    assert!(!kw.contains("in"));
}

#[test]
fn test_keyword_overlap_identical_sets() {
    let set_a = keywords("Missing error context in anyhow chains");
    let set_b = keywords("Missing error context in anyhow chains");
    assert!(keyword_overlap_exceeds(&set_a, &set_b));
}

#[test]
fn test_keyword_overlap_empty_sets() {
    let empty: HashSet<String> = HashSet::new();
    let non_empty = keywords("some words here for testing");
    assert!(!keyword_overlap_exceeds(&empty, &non_empty));
    assert!(!keyword_overlap_exceeds(&non_empty, &empty));
}
