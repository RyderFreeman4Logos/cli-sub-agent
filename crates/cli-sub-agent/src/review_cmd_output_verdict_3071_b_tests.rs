const CLASS_B_DETAILS: &str = r#"# Code Review Report
## Findings

1. [HIGH][security] \\u{6587}\\u{4ef6}\\u{7f16}\\u{8bd1}\\u{5165}\\u{53e3}\\u{53ef}\\u{65e0}\\u{9650}\\u{8bfb}\\u{53d6}\\u{6216}\\u{6c38}\\u{4e45}\\u{963b}\\u{585e}
   `crates/workflow-compiler/src/lib.rs:98`, confidence=0.99

   - Trigger: `workflowctl validate /dev/zero`、FIFO，\\u{6216}\\u{8d85}\\u{5927}\\u{5de5}\\u{4f5c}\\u{6d41}\\u{6587}\\u{4ef6}。
   - Expected: \\u{8f93}\\u{5165}\\u{53d7}\\u{5230}\\u{5927}\\u{5c0f}\\u{548c}\\u{6587}\\u{4ef6}\\u{7c7b}\\u{578b}\\u{9650}\\u{5236}，\\u{5931}\\u{8d25}\\u{65f6}\\u{8f93}\\u{51fa}\\u{7a33}\\u{5b9a} Diagnostic \\u{5e76}\\u{4ee5} 2 \\u{9000}\\u{51fa}。
   - Actual: \\u{65b0}\\u{589e}\\u{7684} `compile_file` \\u{65e0}\\u{6761}\\u{4ef6}\\u{8c03}\\u{7528} `parse_file`；\\u{540e}\\u{8005}\\u{5728} `crates/workflow-spec/src/lib.rs:410` \\u{4f7f}\\u{7528}\\u{65e0}\\u{4e0a}\\u{9650} `std::fs::read`。\\u{5b57}\\u{7b26}\\u{8bbe}\\u{5907}\\u{53ef}\\u{6301}\\u{7eed}\\u{4ea7}\\u{751f}\\u{6570}\\u{636e}\\u{76f4}\\u{81f3} OOM，FIFO \\u{53ef}\\u{65e0}\\u{9650}\\u{7b49}\\u{5f85}。
   - Impact: \\u{653b}\\u{51fb}\\u{8005}\\u{63a7}\\u{5236}\\u{5de5}\\u{4f5c}\\u{6d41}\\u{8def}\\u{5f84}\\u{6216}\\u{4ed3}\\u{5e93}\\u{5185}\\u{5bb9}\\u{65f6}，\\u{53ef}\\u{6302}\\u{8d77}\\u{6216}\\u{8017}\\u{5c3d} CLI/CI \\u{4e3b}\\u{673a}\\u{8d44}\\u{6e90}。
   - AGENTS.md: `Practice 014|security|validate ALL input`
   - Class sweep: 3 sites — `validate`、`graph`、`lock` \\u{5747}\\u{7ecf}\\u{8fc7}\\u{8be5}\\u{5171}\\u{4eab}\\u{5165}\\u{53e3}（`main.rs:121,136,151`）。
   - Fix: \\u{6253}\\u{5f00}\\u{6587}\\u{4ef6}\\u{540e}\\u{68c0}\\u{67e5}\\u{6587}\\u{4ef6}\\u{53e5}\\u{67c4}\\u{5143}\\u{6570}\\u{636e}，\\u{5e76}\\u{901a}\\u{8fc7}\\u{6709}\\u{660e}\\u{786e}\\u{4e0a}\\u{9650}\\u{7684}\\u{8bfb}\\u{53d6}\\u{5668}\\u{8bfb}\\u{53d6} `limit + 1` \\u{5b57}\\u{8282}；\\u{62d2}\\u{7edd}\\u{975e}\\u{666e}\\u{901a}\\u{6587}\\u{4ef6}\\u{548c}\\u{8d85}\\u{9650}\\u{8f93}\\u{5165}，\\u{65b0}\\u{589e}\\u{7a33}\\u{5b9a}\\u{8bca}\\u{65ad}。
   - Test: \\u{7528}\\u{8d85}\\u{9650}\\u{666e}\\u{901a}\\u{6587}\\u{4ef6}\\u{548c} `/dev/zero` \\u{9a8c}\\u{8bc1}\\u{4e09}\\u{4e2a}\\u{547d}\\u{4ee4}\\u{5747}\\u{5728}\\u{73b0}\\u{6709} 5 \\u{79d2}\\u{5b88}\\u{536b}\\u{5185}\\u{4ee5}\\u{8bca}\\u{65ad}\\u{9000}\\u{51fa}。

2. [MEDIUM][regression] `--` \\u{540e}\\u{7684}\\u{8def}\\u{5f84} `--json` \\u{88ab}\\u{9519}\\u{8bef}\\u{8bc6}\\u{522b}\\u{4e3a}\\u{683c}\\u{5f0f}\\u{9009}\\u{9879}
   `crates/workflowctl/src/main.rs:81`, confidence=0.98

   - Trigger: `workflowctl validate -- --json`。
   - Expected: Clap \\u{5c06} `--` \\u{540e}\\u{7684} `--json` \\u{4f5c}\\u{4e3a}\\u{8def}\\u{5f84}；\\u{7f3a}\\u{5931}\\u{6587}\\u{4ef6}\\u{5e94}\\u{4ea7}\\u{751f}\\u{4eba}\\u{7c7b}\\u{53ef}\\u{8bfb}\\u{8bca}\\u{65ad}。
   - Actual: \\u{89e3}\\u{6790}\\u{524d}\\u{7684}\\u{5168}\\u{53c2}\\u{6570}\\u{626b}\\u{63cf}\\u{628a}\\u{4efb}\\u{4f55}\\u{4f4d}\\u{7f6e}\\u{7684} `--json` \\u{90fd}\\u{8bbe}\\u{4e3a} JSON \\u{6a21}\\u{5f0f}，\\u{56e0}\\u{6b64}\\u{8f93}\\u{51fa} JSON \\u{8bca}\\u{65ad}。
   - Impact: \\u{5408}\\u{6cd5}\\u{6587}\\u{4ef6}\\u{540d}\\u{88ab}\\u{8bef}\\u{5206}\\u{7c7b}，\\u{8fdd}\\u{53cd}“`--json` \\u{53ea}\\u{4f5c}\\u{4e3a}\\u{9519}\\u{8bef}\\u{683c}\\u{5f0f}\\u{9009}\\u{9879}”\\u{7684} CLI \\u{5408}\\u{540c}。
   - Class sweep: 3 sites — \\u{6240}\\u{6709}\\u{5e26} `PATH` \\u{7684}\\u{5b50}\\u{547d}\\u{4ee4}\\u{5171}\\u{4eab}\\u{8be5}\\u{626b}\\u{63cf}。
   - Fix: \\u{539f}\\u{59cb}\\u{53c2}\\u{6570}\\u{626b}\\u{63cf}\\u{5fc5}\\u{987b}\\u{5728}\\u{7b2c}\\u{4e00}\\u{4e2a} `--` \\u{5904}\\u{505c}\\u{6b62}。
   - Test: \\u{8986}\\u{76d6}\\u{4e09}\\u{4e2a}\\u{5b50}\\u{547d}\\u{4ee4}\\u{7684} `-- --json` \\u{8def}\\u{5f84}。

3. [MEDIUM][regression] \\u{6210}\\u{529f}\\u{8f93}\\u{51fa}\\u{5199}\\u{5165}\\u{5931}\\u{8d25}\\u{4f1a} panic，\\u{800c}\\u{4e0d}\\u{662f}\\u{9075}\\u{5b88}\\u{9000}\\u{51fa}\\u{5408}\\u{540c}
   `crates/workflowctl/src/main.rs:122`, confidence=0.97

   - Trigger: \\u{5bf9}\\u{6709}\\u{6548}\\u{8f93}\\u{5165}\\u{542f}\\u{52a8}\\u{547d}\\u{4ee4}\\u{540e}\\u{7acb}\\u{5373}\\u{5173}\\u{95ed}\\u{5176} stdout \\u{8bfb}\\u{53d6}\\u{7aef}。
   - Expected: \\u{8f93}\\u{51fa}\\u{5931}\\u{8d25}\\u{5e94}\\u{6210}\\u{4e3a}\\u{53d7}\\u{63a7}\\u{9519}\\u{8bef}；\\u{6309}\\u{5df2}\\u{6279}\\u{51c6}\\u{5408}\\u{540c}\\u{8f93}\\u{51fa} Diagnostic \\u{5e76}\\u{4ee5} 2 \\u{9000}\\u{51fa}。
   - Actual: `println!`/`print!` \\u{5728} stdout \\u{5199}\\u{5165}\\u{9519}\\u{8bef}\\u{65f6} panic，\\u{4ea7}\\u{751f}\\u{975e} 2 \\u{9000}\\u{51fa}\\u{72b6}\\u{6001}\\u{548c}\\u{4e0d}\\u{7a33}\\u{5b9a}\\u{7684} panic \\u{6587}\\u{672c}。
   - Impact: \\u{5e38}\\u{89c1}\\u{7684}\\u{63d0}\\u{524d}\\u{5173}\\u{95ed}\\u{7ba1}\\u{9053}\\u{4f1a}\\u{4f7f}\\u{811a}\\u{672c}\\u{89c2}\\u{5bdf}\\u{5230}\\u{5408}\\u{540c}\\u{5916}\\u{7684}\\u{5d29}\\u{6e83}\\u{884c}\\u{4e3a}。
   - AGENTS.md: `Practice 009|error-handling`
   - Class sweep: 3 sites — `validate`、`graph`、`lock` \\u{7684}\\u{6210}\\u{529f}\\u{8f93}\\u{51fa}\\u{4f4d}\\u{4e8e} `main.rs:122,137,172`。
   - Fix: \\u{4f7f}\\u{7528}\\u{663e}\\u{5f0f} `Write` \\u{8c03}\\u{7528}\\u{5e76}\\u{6295}\\u{5f71}\\u{5199}\\u{5165}\\u{9519}\\u{8bef}。
   - Test: \\u{542f}\\u{52a8}\\u{5b50}\\u{8fdb}\\u{7a0b}\\u{540e}\\u{4e22}\\u{5f03} stdout pipe \\u{7684}\\u{8bfb}\\u{53d6}\\u{7aef}，\\u{5e76}\\u{65ad}\\u{8a00}\\u{53d7}\\u{63a7}\\u{9000}\\u{51fa}。

## Cross-Dimension Blocking Enumeration

1. Security: Finding 1 \\u{662f}\\u{5f53}\\u{524d}\\u{552f}\\u{4e00} HIGH/P1 \\u{963b}\\u{585e}\\u{9879}。
2. Correctness、concurrency、contract/doc-sync、ordering、completeness \\u{672a}\\u{53d1}\\u{73b0}\\u{5176}\\u{4ed6} HIGH/CRITICAL \\u{72ec}\\u{7acb}\\u{7f3a}\\u{9677}；Findings 2–3 \\u{4e3a} MEDIUM，\\u{4f46}\\u{5f53}\\u{524d}\\u{5ba1}\\u{67e5}\\u{95e8}\\u{8981}\\u{6c42}\\u{96f6}\\u{4e25}\\u{91cd}\\u{5ea6}\\u{53d1}\\u{73b0}。
"#;

#[test]
fn issue_3071_class_b_counts_three_frozen_findings_once() {
    let (_env_lock, project_root, session_dir) = persist_fixture_review(
        "issue-3071-class-b",
        "01TEST3071CLASSB000000",
        "\\u{5ba1}\\u{67e5}\\u{53d1}\\u{73b0} 3 \\u{4e2a}\\u{7f3a}\\u{9677}：1 \\u{4e2a} HIGH \\u{8d44}\\u{6e90}\\u{8017}\\u{5c3d}\\u{5b89}\\u{5168}\\u{95ee}\\u{9898}、2 \\u{4e2a} MEDIUM CLI \\u{5408}\\u{540c}\\u{95ee}\\u{9898}。",
        CLASS_B_DETAILS,
        r#"[[findings]]
id = "prose-generated-001"
severity = "high"
description = "\\u{6587}\\u{4ef6}\\u{7f16}\\u{8bd1}\\u{5165}\\u{53e3}\\u{53ef}\\u{65e0}\\u{9650}\\u{8bfb}\\u{53d6}\\u{6216}\\u{6c38}\\u{4e45}\\u{963b}\\u{585e}"

[[findings]]
id = "prose-generated-002"
severity = "medium"
description = "confidence=0.99"
file_ranges = [{ path = "crates/workflow-compiler/src/lib.rs", start = 98 }]

[[findings]]
id = "prose-generated-003"
severity = "medium"
description = "`--` \\u{540e}\\u{7684}\\u{8def}\\u{5f84} `--json` \\u{88ab}\\u{9519}\\u{8bef}\\u{8bc6}\\u{522b}\\u{4e3a}\\u{683c}\\u{5f0f}\\u{9009}\\u{9879}"

[[findings]]
id = "prose-generated-004"
severity = "medium"
description = "confidence=0.98"
file_ranges = [{ path = "crates/workflowctl/src/main.rs", start = 81 }]

[[findings]]
id = "prose-generated-005"
severity = "medium"
description = "\\u{6210}\\u{529f}\\u{8f93}\\u{51fa}\\u{5199}\\u{5165}\\u{5931}\\u{8d25}\\u{4f1a} panic，\\u{800c}\\u{4e0d}\\u{662f}\\u{9075}\\u{5b88}\\u{9000}\\u{51fa}\\u{5408}\\u{540c}"

[[findings]]
id = "prose-generated-006"
severity = "medium"
description = "confidence=0.97"
file_ranges = [{ path = "crates/workflowctl/src/main.rs", start = 122 }]

[[findings]]
id = "prose-generated-007"
severity = "high"
description = "\\u{6587}\\u{4ef6}\\u{7f16}\\u{8bd1}\\u{5165}\\u{53e3}\\u{53ef}\\u{65e0}\\u{9650}\\u{8bfb}\\u{53d6}\\u{6216}\\u{6c38}\\u{4e45}\\u{963b}\\u{585e}"

[[findings]]
id = "prose-generated-008"
severity = "medium"
description = "confidence=0.99"
file_ranges = [{ path = "crates/workflow-compiler/src/lib.rs", start = 98 }]

[[findings]]
id = "prose-generated-009"
severity = "medium"
description = "`--` \\u{540e}\\u{7684}\\u{8def}\\u{5f84} `--json` \\u{88ab}\\u{9519}\\u{8bef}\\u{8bc6}\\u{522b}\\u{4e3a}\\u{683c}\\u{5f0f}\\u{9009}\\u{9879}"

[[findings]]
id = "prose-generated-010"
severity = "medium"
description = "confidence=0.98"
file_ranges = [{ path = "crates/workflowctl/src/main.rs", start = 81 }]

[[findings]]
id = "prose-generated-011"
severity = "medium"
description = "\\u{6210}\\u{529f}\\u{8f93}\\u{51fa}\\u{5199}\\u{5165}\\u{5931}\\u{8d25}\\u{4f1a} panic，\\u{800c}\\u{4e0d}\\u{662f}\\u{9075}\\u{5b88}\\u{9000}\\u{51fa}\\u{5408}\\u{540c}"

[[findings]]
id = "prose-generated-012"
severity = "medium"
description = "confidence=0.97"
file_ranges = [{ path = "crates/workflowctl/src/main.rs", start = 122 }]
"#,
        true,
    );
    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 3, "{findings:#?}");
    assert_eq!(
        findings
            .findings
            .iter()
            .map(|finding| finding.severity.clone())
            .collect::<Vec<_>>(),
        [Severity::High, Severity::Medium, Severity::Medium]
    );
    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&2));
    assert_eq!(verdict.severity_counts.values().sum::<u32>(), 3);
    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_3071_structured_prose_prefixed_id_survives_reconciliation() {
    let (_env_lock, project_root, session_dir) = persist_fixture_review(
        "issue-3071-structured-prose-id",
        "01TEST3071PROSEID00000",
        "Review found one high-severity security finding.",
        "## Findings\n1. [HIGH][security] Active security regression remains.\n",
        r#"[[findings]]
id = "prose-security-1"
severity = "high"
description = "Structured security finding"
"#,
        true,
    );

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2, "{findings:#?}");
    assert!(
        findings
            .findings
            .iter()
            .any(|finding| finding.id == "prose-security-1"),
        "structured prose-prefixed finding was removed: {findings:#?}"
    );
    assert!(
        findings
            .findings
            .iter()
            .any(|finding| finding.description == "Active security regression remains."),
        "parsed prose finding was removed: {findings:#?}"
    );

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&2));
    assert_eq!(verdict.severity_counts.values().sum::<u32>(), 2);
    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_3071_structured_numeric_prose_id_survives_reconciliation() {
    let (_env_lock, project_root, session_dir) = persist_fixture_review(
        "issue-3071-structured-numeric-prose-id",
        "01TEST3071NUMERICID0000",
        "Review found one high-severity security finding.",
        "## Findings\n1. [HIGH][security] Parsed security regression remains.\n",
        r#"[[findings]]
id = "prose-123"
severity = "high"
description = "Structured numeric prose ID"
"#,
        true,
    );

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2, "{findings:#?}");
    assert!(
        findings
            .findings
            .iter()
            .any(|finding| finding.id == "prose-123"),
        "structured numeric prose finding was removed: {findings:#?}"
    );
    assert!(
        findings
            .findings
            .iter()
            .any(|finding| finding.description == "Parsed security regression remains."),
        "parsed prose finding was removed: {findings:#?}"
    );

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&2));
    assert_eq!(verdict.severity_counts.values().sum::<u32>(), 2);
    fs::remove_dir_all(project_root).expect("remove temp project root");
}
