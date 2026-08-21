#[test]
fn issue_3071_class_c_counts_one_frozen_finding_once() {
    let (_env_lock, project_root, session_dir) = persist_fixture_review(
        "issue-3071-class-c",
        "01TEST3071CLASSC000000",
        "\\u{53d1}\\u{73b0} 1 \\u{4e2a} HIGH/P1 \\u{5b89}\\u{5168}\\u{7f3a}\\u{9677}：\\u{6587}\\u{4ef6}\\u{7c7b}\\u{578b}\\u{68c0}\\u{67e5}\\u{4e0e}\\u{6253}\\u{5f00}\\u{5b58}\\u{5728} TOCTOU。",
        r#"# Code Review Report
## Findings
1. [P1][security] \\u{6587}\\u{4ef6}\\u{7c7b}\\u{578b}\\u{68c0}\\u{67e5}\\u{4e0e}\\u{6253}\\u{5f00}\\u{4e0d}\\u{662f}\\u{540c}\\u{4e00}\\u{6587}\\u{4ef6}\\u{5bf9}\\u{8c61}（`crates/workflow-spec/src/lib.rs:423`，confidence=0.99）

   - Trigger: \\u{6709}\\u{6743}\\u{4fee}\\u{6539}\\u{7236}\\u{76ee}\\u{5f55}\\u{7684}\\u{8fdb}\\u{7a0b}\\u{5728} `metadata(path)` \\u{8fd4}\\u{56de}\\u{540e}、`File::open(path)` \\u{6267}\\u{884c}\\u{524d}，\\u{5c06}\\u{666e}\\u{901a}\\u{6587}\\u{4ef6}\\u{66ff}\\u{6362}\\u{4e3a}\\u{65e0}\\u{5199}\\u{5165}\\u{7aef}\\u{7684} FIFO。
   - Expected: \\u{975e}\\u{666e}\\u{901a}\\u{6587}\\u{4ef6}\\u{5fc5}\\u{987b}\\u{5728}\\u{4e0d}\\u{963b}\\u{585e}\\u{7684}\\u{60c5}\\u{51b5}\\u{4e0b}\\u{88ab}\\u{62d2}\\u{7edd}，\\u{4e14}\\u{6821}\\u{9a8c}\\u{5e94}\\u{7ed1}\\u{5b9a}\\u{5230}\\u{5b9e}\\u{9645}\\u{8bfb}\\u{53d6}\\u{7684}\\u{6587}\\u{4ef6}\\u{63cf}\\u{8ff0}\\u{7b26}。
   - Actual: \\u{7b2c} 423–425 \\u{884c}\\u{68c0}\\u{67e5}\\u{7b2c}\\u{4e00}\\u{6b21}\\u{8def}\\u{5f84}\\u{89e3}\\u{6790}，\\u{7b2c} 435–436 \\u{884c}\\u{518d}\\u{6b21}\\u{89e3}\\u{6790}\\u{8def}\\u{5f84}；\\u{7b2c}\\u{4e8c}\\u{6b21}\\u{6253}\\u{5f00} FIFO \\u{4f1a}\\u{65e0}\\u{9650}\\u{7b49}\\u{5f85}。
   - Impact: \\u{6062}\\u{590d}\\u{4e86}\\u{4e0a}\\u{4e00}\\u{8f6e}\\u{8981}\\u{6c42}\\u{6d88}\\u{9664}\\u{7684}\\u{6c38}\\u{4e45}\\u{963b}\\u{585e}/\\u{62d2}\\u{7edd}\\u{670d}\\u{52a1}。
   - Class sweep: 3 sites—`validate`、`graph`、`lock` \\u{5747}\\u{7ecf} `compile_file` \\u{5230}\\u{8fbe}\\u{8be5}\\u{5171}\\u{4eab}\\u{8bfb}\\u{53d6}\\u{51fd}\\u{6570}。
   - Fix：\\u{5728} Linux \\u{4e0a}\\u{4ee5} nonblocking \\u{6a21}\\u{5f0f}\\u{6253}\\u{5f00}\\u{4e00}\\u{6b21}，\\u{518d}\\u{7528} `file.metadata()` \\u{68c0}\\u{67e5}\\u{540c}\\u{4e00}\\u{63cf}\\u{8ff0}\\u{7b26}，\\u{968f}\\u{540e}\\u{4ece}\\u{540c}\\u{4e00}\\u{63cf}\\u{8ff0}\\u{7b26}\\u{6267}\\u{884c}\\u{9650}\\u{957f}\\u{8bfb}\\u{53d6}。
   - Test gap: \\u{5f53}\\u{524d} `/dev/null` \\u{6d4b}\\u{8bd5}\\u{53ea}\\u{8986}\\u{76d6}\\u{7a33}\\u{5b9a}\\u{7684}\\u{975e}\\u{666e}\\u{901a}\\u{6587}\\u{4ef6}，\\u{6ca1}\\u{6709}\\u{8986}\\u{76d6}\\u{68c0}\\u{67e5}\\u{4e0e}\\u{6253}\\u{5f00}\\u{4e4b}\\u{95f4}\\u{7684}\\u{8def}\\u{5f84}\\u{66ff}\\u{6362}。
"#,
        r#"[[findings]]
id = "prose-generated-001"
severity = "high"
description = "\\u{6587}\\u{4ef6}\\u{7c7b}\\u{578b}\\u{68c0}\\u{67e5}\\u{4e0e}\\u{6253}\\u{5f00}\\u{4e0d}\\u{662f}\\u{540c}\\u{4e00}\\u{6587}\\u{4ef6}\\u{5bf9}\\u{8c61}（`crates/workflow-spec/src/lib.rs:423`，confidence=0.99）"

[[findings]]
id = "prose-generated-002"
severity = "high"
description = "\\u{6587}\\u{4ef6}\\u{7c7b}\\u{578b}\\u{68c0}\\u{67e5}\\u{4e0e}\\u{6253}\\u{5f00}\\u{4e0d}\\u{662f}\\u{540c}\\u{4e00}\\u{6587}\\u{4ef6}\\u{5bf9}\\u{8c61}（`crates/workflow-spec/src/lib.rs:423`，confidence=0.99）"
"#,
        true,
    );
    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1, "{findings:#?}");
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges,
        vec![csa_session::ReviewFindingFileRange {
            path: "crates/workflow-spec/src/lib.rs".to_string(),
            start: 423,
            end: None,
        }]
    );
    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.values().sum::<u32>(), 1);
    fs::remove_dir_all(project_root).expect("remove temp project root");
}
