#[test]
fn issue_1754_findings_toml_derives_medium_path_prefixed_prose_finding() {
    let project_root = temp_project_root("issue-1754-medium-prose-finding");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TEST1754MEDIUMPROSE000");
    let session_dir = create_session_dir(&project_root, &session_id);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review found one issue.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Medium: docs/debate-review.md:151 reviewer output can hide the required follow-up.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(parsed.findings.len(), 1);
    assert_eq!(parsed.findings[0].severity, Severity::Medium);
    assert_eq!(
        parsed.findings[0].file_ranges[0].path,
        "docs/debate-review.md"
    );
    assert_eq!(parsed.findings[0].file_ranges[0].start, 151);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_reads_output_log_when_full_md_is_missing() {
    let project_root = temp_project_root("persist-review-findings-output-log");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSTOMLOUTPUTLOG");
    let session_dir = create_session_dir(&project_root, &session_id);
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text": r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
One issue found.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "f-output-log"
severity = "high"
description = "Findings fence lives outside structured sections."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 88
```"#
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output.log"), full_output).expect("write output.log");
    fs::write(
        session_dir.join("output").join("details.md"),
        "One issue found.\n",
    )
    .expect("write details.md");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "f-output-log",
                Severity::High,
                "crates/cli-sub-agent/src/review_cmd_findings_toml.rs",
                88,
                "Findings fence lives outside structured sections."
            )],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_writes_synthetic_empty_for_findings_fence_without_toml_extension() {
    let project_root = temp_project_root("persist-review-findings-no-extension");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSTOMLNOEXT00");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"```findings
[[findings]]
id = "f1"
severity = "high"
description = "This fence should be ignored."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 130
```"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "Ignored findings fence.\n",
    )
    .expect("write details.md");

    let meta = make_review_meta(&session_id);
    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse synthetic findings.toml");
    assert_eq!(parsed, FindingsFile::default());
    assert_eq!(actual.trim(), "findings = []");
    assert!(buffer.contents().contains(
        "Reviewer findings.toml block missing or invalid; wrote synthetic empty artifact"
    ));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_writes_synthetic_empty_when_block_missing() {
    let project_root = temp_project_root("persist-review-findings-toml-empty");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSTOMLEMPTY000");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
No blocking issues found.
<!-- CSA:SECTION:details:END -->
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "No blocking issues found.\n",
    )
    .expect("write details.md");

    let meta = make_review_meta(&session_id);
    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse synthetic findings.toml");
    assert_eq!(parsed, FindingsFile::default());
    assert_eq!(actual.trim(), "findings = []");
    assert!(buffer.contents().contains(
        "Reviewer findings.toml block missing or invalid; wrote synthetic empty artifact"
    ));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_overwrites_existing_empty_artifact() {
    let project_root = temp_project_root("persist-review-findings-overwrite-empty");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSEMPTYOVERWR0");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
One issue found.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "new-f1"
severity = "medium"
description = "Replace the empty placeholder artifact."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 31
```
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "One issue found.\n",
    )
    .expect("write details.md");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "new-f1",
                Severity::Medium,
                "crates/cli-sub-agent/src/review_cmd_findings_toml.rs",
                31,
                "Replace the empty placeholder artifact.",
            )],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_overwrites_existing_findings_toml_with_new_content() {
    let project_root = temp_project_root("persist-review-findings-overwrite-existing");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSNONEMPTYOVR0");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
One fresh issue found.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "fresh-f1"
severity = "medium"
description = "Fresh reviewer output should replace prior findings."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 27
```
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "One fresh issue found.\n",
    )
    .expect("write details.md");

    let existing = FindingsFile {
        findings: vec![sample_finding(
            "existing-f1",
            Severity::High,
            "crates/cli-sub-agent/src/review_cmd_output.rs",
            173,
            "Old reviewer output should not be preserved.",
        )],
    };
    fs::write(
        session_dir.join("output").join("findings.toml"),
        toml::to_string(&existing).expect("serialize existing findings"),
    )
    .expect("write existing findings.toml");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "fresh-f1",
                Severity::Medium,
                "crates/cli-sub-agent/src/review_cmd_findings_toml.rs",
                27,
                "Fresh reviewer output should replace prior findings.",
            )],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_overwrites_existing_empty_artifact_with_derived_empty() {
    let project_root = temp_project_root("persist-review-findings-empty-over-empty");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSEMPTYEMPTY00");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
No blocking issues found.
<!-- CSA:SECTION:details:END -->
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "No blocking issues found.\n",
    )
    .expect("write details.md");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read empty findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse empty findings.toml");
    assert_eq!(parsed, FindingsFile::default());
    assert_eq!(actual.trim(), "findings = []");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_overwrites_unparseable_existing_artifact_with_derived_empty() {
    let project_root = temp_project_root("persist-review-findings-empty-over-garbage");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSEMPTYGARBAG0");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
No blocking issues found.
<!-- CSA:SECTION:details:END -->
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "No blocking issues found.\n",
    )
    .expect("write details.md");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "this is not toml\n[[\n",
    )
    .expect("write invalid findings.toml");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(parsed, FindingsFile::default());
    assert_eq!(actual.trim(), "findings = []");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_overwrites_unparseable_existing_artifact() {
    let project_root = temp_project_root("persist-review-findings-overwrite-garbage");
    let _state_home = pin_state_home(&project_root);
    let session_id = unique_session_id("01TESTFINDINGSGARBAGEOVR0");
    let session_dir = create_session_dir(&project_root, &session_id);
    write_review_full_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
One issue found.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "new-f1"
severity = "low"
description = "Replace the corrupt artifact."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml_tests.rs"
start = 300
```
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "One issue found.\n",
    )
    .expect("write details.md");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "this is not toml\n[[\n",
    )
    .expect("write invalid findings.toml");

    let meta = make_review_meta(&session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "new-f1",
                Severity::Low,
                "crates/cli-sub-agent/src/review_cmd_findings_toml_tests.rs",
                300,
                "Replace the corrupt artifact.",
            )],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2536_explicit_findings_none_suppresses_prose_pseudo_findings() {
    // When details.md says "Findings: none." but the review text contains
    // support/evidence prose like "P1 supported by justfile:192-195", those
    // prose lines should NOT be converted into blocking findings (#2536).
    let review_text = "Findings: none.\n\n1. P1 supported by `justfile: 192-195`.";

    assert!(
        review_explicitly_states_no_findings(review_text),
        "review text explicitly states no findings"
    );

    let review_without_none = "1. [HIGH] Something is wrong with foo.rs:42";
    assert!(
        !review_explicitly_states_no_findings(review_without_none),
        "review without explicit none should not be suppressed"
    );
}
