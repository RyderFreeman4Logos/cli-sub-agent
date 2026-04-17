use super::{extract_findings_toml_from_text, persist_review_findings_toml};
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{FindingSeverity, FindingsFile, ReviewFinding, ReviewFindingFileRange};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::fmt::MakeWriter;

fn make_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    }
}

fn temp_project_root(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
    fs::create_dir_all(&path).expect("create temp project root");
    path
}

fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
    let session_dir =
        csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    session_dir
}

fn write_review_full_output(session_dir: &Path, review_text: &str) {
    let full_output = [json!({"type":"item.completed","item":{
        "id":"item_1",
        "type":"agent_message",
        "text": review_text
    }})]
    .into_iter()
    .map(|line| serde_json::to_string(&line).expect("serialize transcript line"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(session_dir.join("output").join("full.md"), full_output)
        .expect("write full output transcript");
}

fn sample_finding(
    id: &str,
    severity: FindingSeverity,
    path: &str,
    start: u32,
    description: &str,
) -> ReviewFinding {
    ReviewFinding {
        id: id.to_string(),
        severity,
        file_ranges: vec![ReviewFindingFileRange {
            path: path.to_string(),
            start,
            end: None,
        }],
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: description.to_string(),
    }
}

#[derive(Clone, Default)]
struct SharedLogBuffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.bytes.lock().expect("lock log buffer").clone())
            .expect("buffer should contain valid utf-8")
    }
}

struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("lock log writer")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn extract_findings_toml_from_text_prefers_labeled_block() {
    let review_text = r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Human-readable details stay here.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "f1"
severity = "high"
description = "Regression drops the retry path."
is_regression_of_commit = "29b6c34c"
suggested_test_scenario = "Retry the failed review once."

[[findings.file_ranges]]
path = "crates/foo/src/bar.rs"
start = 73
end = 80
```
"#;

    let parsed =
        extract_findings_toml_from_text(review_text).expect("findings.toml block should parse");

    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![ReviewFinding {
                id: "f1".to_string(),
                severity: FindingSeverity::High,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "crates/foo/src/bar.rs".to_string(),
                    start: 73,
                    end: Some(80),
                }],
                is_regression_of_commit: Some("29b6c34c".to_string()),
                suggested_test_scenario: Some("Retry the failed review once.".to_string()),
                description: "Regression drops the retry path.".to_string(),
            }],
        }
    );
}

#[test]
fn extract_findings_toml_from_text_accepts_single_token_findings_toml_fence() {
    let review_text = r#"```findings.toml
[[findings]]
id = "f1"
severity = "high"
description = "Regression drops the retry path."

[[findings.file_ranges]]
path = "crates/foo/src/bar.rs"
start = 73
end = 80
```"#;

    let parsed =
        extract_findings_toml_from_text(review_text).expect("findings.toml block should parse");

    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![ReviewFinding {
                id: "f1".to_string(),
                severity: FindingSeverity::High,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "crates/foo/src/bar.rs".to_string(),
                    start: 73,
                    end: Some(80),
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Regression drops the retry path.".to_string(),
            }],
        }
    );
}

#[test]
fn extract_findings_toml_from_text_returns_none_without_block() {
    let review_text = r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
No blocking issues found.
<!-- CSA:SECTION:details:END -->
"#;

    assert!(extract_findings_toml_from_text(review_text).is_none());
}

#[test]
fn extract_findings_toml_from_text_rejects_unrelated_toml_fence() {
    let review_text = r#"```toml
[some_other_section]
key = "value"
```"#;

    assert!(extract_findings_toml_from_text(review_text).is_none());
}

#[test]
fn extract_findings_toml_from_text_rejects_findings_fence_without_toml_extension() {
    let review_text = r#"```findings
[[findings]]
id = "f1"
severity = "high"
description = "Regression drops the retry path."

[[findings.file_ranges]]
path = "crates/foo/src/bar.rs"
start = 73
```"#;

    assert!(extract_findings_toml_from_text(review_text).is_none());
}

#[test]
fn extract_findings_toml_from_text_prefers_findings_toml_over_generic_toml() {
    let review_text = r#"```toml
findings = []
```

```toml findings.toml
[[findings]]
id = "f1"
severity = "medium"
description = "Use the labeled findings block."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 101
```"#;

    let parsed =
        extract_findings_toml_from_text(review_text).expect("findings.toml block should parse");

    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![ReviewFinding {
                id: "f1".to_string(),
                severity: FindingSeverity::Medium,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "crates/cli-sub-agent/src/review_cmd_findings_toml.rs".to_string(),
                    start: 101,
                    end: None,
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Use the labeled findings block.".to_string(),
            }],
        }
    );
}

#[test]
fn persist_review_findings_toml_writes_parsed_artifact() {
    let project_root = temp_project_root("persist-review-findings-toml");
    let session_id = "01TESTFINDINGSTOML00000000";
    let session_dir = create_session_dir(&project_root, session_id);
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
id = "f1"
severity = "medium"
description = "Missing regression coverage."
suggested_test_scenario = "Run the fixer on an already reviewed branch."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd.rs"
start = 425
```
"#,
    );
    fs::write(
        session_dir.join("output").join("details.md"),
        "One issue found.\n",
    )
    .expect("write details.md");

    let meta = make_review_meta(session_id);
    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![ReviewFinding {
                id: "f1".to_string(),
                severity: FindingSeverity::Medium,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "crates/cli-sub-agent/src/review_cmd.rs".to_string(),
                    start: 425,
                    end: None,
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: Some(
                    "Run the fixer on an already reviewed branch.".to_string()
                ),
                description: "Missing regression coverage.".to_string(),
            }],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn persist_review_findings_toml_reads_output_log_when_full_md_is_missing() {
    let project_root = temp_project_root("persist-review-findings-output-log");
    let session_id = "01TESTFINDINGSTOMLOUTPUTLOG";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
    persist_review_findings_toml(&project_root, &meta);

    let findings_path = session_dir.join("output").join("findings.toml");
    let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "f-output-log",
                FindingSeverity::High,
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
    let session_id = "01TESTFINDINGSTOMLNOEXT00";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
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
    let session_id = "01TESTFINDINGSTOMLEMPTY000";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
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
    let session_id = "01TESTFINDINGSEMPTYOVERWR0";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "new-f1",
                FindingSeverity::Medium,
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
    let session_id = "01TESTFINDINGSNONEMPTYOVR0";
    let session_dir = create_session_dir(&project_root, session_id);
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
            FindingSeverity::High,
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

    let meta = make_review_meta(session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "fresh-f1",
                FindingSeverity::Medium,
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
    let session_id = "01TESTFINDINGSEMPTYEMPTY00";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
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
    let session_id = "01TESTFINDINGSEMPTYGARBAG0";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
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
    let session_id = "01TESTFINDINGSGARBAGEOVR0";
    let session_dir = create_session_dir(&project_root, session_id);
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

    let meta = make_review_meta(session_id);
    persist_review_findings_toml(&project_root, &meta);

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read overwritten findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse overwritten findings.toml");
    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![sample_finding(
                "new-f1",
                FindingSeverity::Low,
                "crates/cli-sub-agent/src/review_cmd_findings_toml_tests.rs",
                300,
                "Replace the corrupt artifact.",
            )],
        }
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
