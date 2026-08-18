#[test]
fn issue_3071_class_a_replaces_artifact_placeholder_with_source_located_finding() {
    let (_env_lock, project_root, session_dir) = persist_fixture_review(
        "issue-3071-class-a",
        "01TEST3071CLASSA000000",
        "\\u{53d1}\\u{73b0} 1 \\u{9879} MEDIUM/P2 \\u{95ee}\\u{9898}：CLI \\u{5951}\\u{7ea6}\\u{6d4b}\\u{8bd5}\\u{7684}\\u{5b50}\\u{8fdb}\\u{7a0b}\\u{6ca1}\\u{6709}\\u{8d85}\\u{65f6}。\\u{7ed9}\\u{5b9a}\\u{4e0a}\\u{4e0b}\\u{6587}\\u{8981}\\u{6c42}\\u{96f6} findings，\\u{56e0}\\u{6b64}\\u{4e0d}\\u{80fd}\\u{901a}\\u{8fc7}。",
        r#"## Finding
1. **[MEDIUM/P2][test-gap] \\u{5b50}\\u{8fdb}\\u{7a0b}\\u{6d4b}\\u{8bd5}\\u{53ef}\\u{80fd}\\u{65e0}\\u{9650}\\u{6302}\\u{8d77}**
   `crates/workflowctl/tests/cli_contracts.rs:13`

   - Trigger：`workflowctl` \\u{56de}\\u{5f52}\\u{4e3a}\\u{6b7b}\\u{5faa}\\u{73af}、\\u{6b7b}\\u{9501}，\\u{6216}\\u{5176}\\u{540e}\\u{4ee3}\\u{8fdb}\\u{7a0b}\\u{6301}\\u{7eed}\\u{6301}\\u{6709}\\u{8f93}\\u{51fa}\\u{7ba1}\\u{9053}。
   - Expected：\\u{6d4b}\\u{8bd5}\\u{5728}\\u{6709}\\u{754c}\\u{671f}\\u{9650}\\u{540e}\\u{6740}\\u{6b7b}\\u{5e76}\\u{56de}\\u{6536}\\u{5b50}\\u{8fdb}\\u{7a0b}，\\u{7136}\\u{540e}\\u{62a5}\\u{544a}\\u{5931}\\u{8d25}。
   - Actual：\\u{5171}\\u{4eab}\\u{52a9}\\u{624b}\\u{76f4}\\u{63a5}\\u{8c03}\\u{7528} `Command::output()`，\\u{6ca1}\\u{6709}\\u{8d85}\\u{65f6}\\u{6216}\\u{6e05}\\u{7406}\\u{8def}\\u{5f84}。
   - Impact：`just test`、\\u{672c}\\u{5730}\\u{8d28}\\u{91cf}\\u{95e8}\\u{548c}\\u{63d0}\\u{4ea4}\\u{94a9}\\u{5b50}\\u{53ef}\\u{80fd}\\u{6c38}\\u{4e45}\\u{7b49}\\u{5f85}。
   - Evidence：\\u{56db}\\u{4e2a}\\u{6d4b}\\u{8bd5}\\u{7684} 21 \\u{6b21}\\u{8fdb}\\u{7a0b}\\u{6267}\\u{884c}\\u{5747}\\u{7ecf}\\u{8fc7}\\u{8be5}\\u{8c03}\\u{7528}。
   - Class sweep：1 \\u{4e2a}\\u{5171}\\u{4eab}\\u{5b9e}\\u{73b0}\\u{4f4d}\\u{7f6e}。
   - Rules：`Rust 015 subprocess-lifecycle`、`bug_category.subprocess_timeout`。
   - Fix：\\u{5728}\\u{73b0}\\u{6709} `output()` \\u{52a9}\\u{624b}\\u{4e2d}\\u{96c6}\\u{4e2d}\\u{5b9e}\\u{73b0}\\u{6709}\\u{754c} spawn/wait/kill/reap，\\u{5e76}\\u{7ee7}\\u{7eed}\\u{5b8c}\\u{6574}\\u{6536}\\u{96c6} stdout/stderr。
   - Confidence：0.99。"#,
        r#"[[findings]]
 id = "artifact-generation-001"
 severity = "medium"
 file_ranges = []
 description = "Artifact generation failed: review verdict is FAIL but CSA could not extract a structured finding. Reason: fail_verdict_empty_findings_artifact. Inspect output/details.md and output/review-verdict.json."
"#,
        false,
    );
    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1, "{findings:#?}");
    assert_eq!(findings.findings[0].severity, Severity::Medium);
    assert_eq!(
        findings.findings[0].file_ranges,
        vec![csa_session::ReviewFindingFileRange {
            path: "crates/workflowctl/tests/cli_contracts.rs".to_string(),
            start: 13,
            end: None,
        }]
    );
    assert!(
        !findings.findings[0]
            .description
            .starts_with("Artifact generation failed:")
    );
    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));
    assert_eq!(verdict.severity_counts.values().sum::<u32>(), 1);
    fs::remove_dir_all(project_root).expect("remove temp project root");
}
