use csa_session::ReviewDiffSize;

use super::*;

fn file(path: &str, changed_lines: usize) -> ReviewChunkFile {
    ReviewChunkFile {
        path: path.to_string(),
        status: "M".to_string(),
        changed_lines,
    }
}

fn large_diff_size(files: usize, changed_lines: usize) -> ReviewDiffSize {
    ReviewDiffSize {
        files,
        changed_lines,
        bytes: 128 * 1024,
        notes: Vec::new(),
    }
}

#[test]
fn small_diff_auto_bypasses_chunking() {
    let config = ReviewChunkingConfig::default();
    let size = ReviewDiffSize {
        files: 3,
        changed_lines: 120,
        bytes: 4096,
        notes: Vec::new(),
    };

    assert_eq!(activation_reason(Some(&size), &config), None);
}

#[test]
fn planner_keeps_crate_groups_before_balancing() {
    let config = ReviewChunkingConfig {
        mode: ReviewChunkingMode::Always,
        ..ReviewChunkingConfig::default()
    };
    let files = vec![
        file("crates/alpha/src/lib.rs", 300),
        file("crates/alpha/src/review.rs", 200),
        file("crates/beta/src/lib.rs", 300),
        file("crates/beta/src/review.rs", 200),
    ];

    let plan = plan_review_chunks_from_files(
        "range:main...HEAD",
        Some(&large_diff_size(4, 1_000)),
        files,
        ReviewChunkActivationReason::Always,
        &config,
    );

    assert_eq!(plan.chunk_count(), 2);
    assert_eq!(plan.chunks[0].group, "crates/alpha");
    assert_eq!(plan.chunks[1].group, "crates/beta");
    assert_eq!(
        plan.chunks[0]
            .pathspecs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["crates/alpha/src/lib.rs", "crates/alpha/src/review.rs"]
    );
}

#[test]
fn planner_caps_total_chunks_without_dropping_files() {
    let config = ReviewChunkingConfig {
        mode: ReviewChunkingMode::Always,
        max_chunks: 12,
        ..ReviewChunkingConfig::default()
    };
    let files = (0..20)
        .map(|idx| file(&format!("crates/c{idx}/src/lib.rs"), 701))
        .collect::<Vec<_>>();

    let plan = plan_review_chunks_from_files(
        "range:main...HEAD",
        Some(&large_diff_size(20, 14_020)),
        files,
        ReviewChunkActivationReason::Always,
        &config,
    );

    assert_eq!(plan.chunk_count(), 12);
    assert_eq!(plan.total_files, 20);
}

#[test]
fn activation_threshold_matches_issue_1816_boundaries() {
    let config = ReviewChunkingConfig::default();
    let by_files = ReviewDiffSize {
        files: 20,
        changed_lines: 1,
        bytes: 1,
        notes: Vec::new(),
    };
    let by_lines = ReviewDiffSize {
        files: 1,
        changed_lines: 1_001,
        bytes: 1,
        notes: Vec::new(),
    };
    let by_bytes = ReviewDiffSize {
        files: 1,
        changed_lines: 1,
        bytes: 80 * 1024 + 1,
        notes: Vec::new(),
    };

    assert_eq!(
        activation_reason(Some(&by_files), &config),
        Some(ReviewChunkActivationReason::FileCount)
    );
    assert_eq!(
        activation_reason(Some(&by_lines), &config),
        Some(ReviewChunkActivationReason::ChangedLines)
    );
    assert_eq!(
        activation_reason(Some(&by_bytes), &config),
        Some(ReviewChunkActivationReason::DiffBytes)
    );
}

#[test]
fn numstat_rename_paths_normalize_to_destination() {
    assert_eq!(
        normalize_numstat_path("crates/app/src/{old.rs => new.rs}"),
        "crates/app/src/new.rs"
    );
    assert_eq!(normalize_numstat_path("old.rs => new.rs"), "new.rs");
}

#[test]
fn name_status_uses_destination_for_renames() {
    let statuses = parse_name_status_output("R100\told.rs\tnew.rs\nM\tlib.rs\n");

    assert_eq!(statuses.get("new.rs").map(String::as_str), Some("R"));
    assert_eq!(statuses.get("lib.rs").map(String::as_str), Some("M"));
}

#[test]
fn config_helpers_and_bypass_predicate_match_cli_semantics() {
    let config = ReviewChunkingConfig::for_args(ReviewChunkingMode::Always);

    assert_eq!(config.mode, ReviewChunkingMode::Always);
    assert_eq!(config.concurrency(), 3);
    assert!(should_bypass_chunking(
        ReviewChunkingMode::Off,
        false,
        false
    ));
    assert!(should_bypass_chunking(
        ReviewChunkingMode::Auto,
        true,
        false
    ));
    assert!(should_bypass_chunking(
        ReviewChunkingMode::Auto,
        false,
        true
    ));
    assert!(!should_bypass_chunking(
        ReviewChunkingMode::Auto,
        false,
        false
    ));
}

#[test]
fn duplicate_findings_across_chunks_collapse_deterministically() {
    let findings = crate::review_consensus::consolidate_findings(vec![
        csa_session::review_artifact::Finding {
            severity: csa_session::review_artifact::Severity::Low,
            fid: "same-finding".to_string(),
            file: "crates/app/src/lib.rs".to_string(),
            line: Some(12),
            rule_id: "rule".to_string(),
            summary: "lower severity duplicate".to_string(),
            engine: "chunk-1".to_string(),
        },
        csa_session::review_artifact::Finding {
            severity: csa_session::review_artifact::Severity::High,
            fid: "same-finding".to_string(),
            file: "crates/app/src/lib.rs".to_string(),
            line: Some(12),
            rule_id: "rule".to_string(),
            summary: "higher severity duplicate".to_string(),
            engine: "chunk-2".to_string(),
        },
    ]);

    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].severity,
        csa_session::review_artifact::Severity::High
    );
    assert_eq!(findings[0].summary, "higher severity duplicate");
}

#[test]
fn unavailable_chunk_fails_closed_when_other_chunks_are_clean() {
    let outcomes = vec![
        ReviewerOutcome {
            reviewer_index: 0,
            tool: ToolName::Codex,
            session_id: "01CLEANCHUNK".to_string(),
            output: "PASS\n".to_string(),
            exit_code: 0,
            verdict: CLEAN,
            diagnostic: None,
        },
        ReviewerOutcome {
            reviewer_index: 1,
            tool: ToolName::Codex,
            session_id: "01UNAVAILABLECHUNK".to_string(),
            output: "UNAVAILABLE\n".to_string(),
            exit_code: 1,
            verdict: UNAVAILABLE,
            diagnostic: Some("provider unavailable".to_string()),
        },
    ];

    assert_eq!(final_chunked_verdict(&outcomes, false), HAS_ISSUES);
}

#[test]
fn chunked_review_audit_writes_parent_output_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let startup_env = StartupSubtreeEnv::default()
        .with_current_session("01PARENTCHUNKED", temp.path().display().to_string());
    let config = ReviewChunkingConfig {
        mode: ReviewChunkingMode::Always,
        ..ReviewChunkingConfig::default()
    };
    let plan = plan_review_chunks_from_files(
        "range:main...HEAD",
        Some(&large_diff_size(2, 200)),
        vec![
            file("crates/alpha/src/lib.rs", 100),
            file("crates/beta/src/lib.rs", 100),
        ],
        ReviewChunkActivationReason::Always,
        &config,
    );
    let outcomes = plan
        .chunks
        .iter()
        .enumerate()
        .map(|(idx, _)| ReviewerOutcome {
            reviewer_index: idx,
            tool: ToolName::Codex,
            session_id: format!("01CHUNK{idx}"),
            output: "PASS\n".to_string(),
            exit_code: 0,
            verdict: CLEAN,
            diagnostic: None,
        })
        .collect::<Vec<_>>();

    write_chunked_review_audit(
        temp.path(),
        &startup_env,
        &plan,
        &outcomes,
        Some("fingerprint".to_string()),
        CLEAN,
        false,
    )
    .expect("write chunked review audit");

    let raw = std::fs::read_to_string(temp.path().join("output").join("chunked-review.json"))
        .expect("read chunked audit");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse chunked audit");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["scope"], "range:main...HEAD");
    assert_eq!(value["final_verdict"], CLEAN);
    assert_eq!(
        value["chunks"].as_array().expect("chunks array").len(),
        plan.chunk_count()
    );
}

#[test]
fn plan_review_chunks_collects_uncommitted_git_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    run_git_test(temp.path(), &["init"]);
    run_git_test(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git_test(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::create_dir_all(temp.path().join("crates/alpha/src")).expect("alpha dir");
    std::fs::create_dir_all(temp.path().join("crates/beta/src")).expect("beta dir");
    std::fs::write(
        temp.path().join("crates/alpha/src/lib.rs"),
        "fn alpha() {}\n",
    )
    .expect("write alpha baseline");
    std::fs::write(temp.path().join("crates/beta/src/lib.rs"), "fn beta() {}\n")
        .expect("write beta baseline");
    run_git_test(temp.path(), &["add", "."]);
    run_git_test(temp.path(), &["commit", "-m", "baseline"]);
    std::fs::write(
        temp.path().join("crates/alpha/src/lib.rs"),
        "fn alpha() {}\nfn alpha_two() {}\n",
    )
    .expect("write alpha change");
    std::fs::write(
        temp.path().join("crates/beta/src/lib.rs"),
        "fn beta() {}\nfn beta_two() {}\n",
    )
    .expect("write beta change");

    let config = ReviewChunkingConfig {
        mode: ReviewChunkingMode::Always,
        ..ReviewChunkingConfig::default()
    };
    let plan = plan_review_chunks(
        temp.path(),
        "uncommitted",
        Some(&large_diff_size(2, 2)),
        &config,
    )
    .expect("chunk planning succeeds")
    .expect("always mode plans chunks");

    let paths = plan
        .chunks
        .iter()
        .flat_map(|chunk| chunk.pathspecs.iter().map(String::as_str))
        .collect::<Vec<_>>();
    assert_eq!(plan.total_files, 2);
    assert!(paths.contains(&"crates/alpha/src/lib.rs"));
    assert!(paths.contains(&"crates/beta/src/lib.rs"));
}

fn run_git_test(project_root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
