use super::persist_fix_final_artifacts;
use csa_core::types::ReviewDecision;
use csa_session::FindingsFile;
use csa_session::state::ReviewSessionMeta;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        tool: "codex".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 0,
        fix_attempted: true,
        fix_rounds: 1,
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

#[test]
fn persist_fix_final_artifacts_writes_findings_toml_from_final_review_output() {
    let project_root = temp_project_root("persist-fix-final-artifacts");
    let session_id = "01TESTFIXFINAL000000000000";
    let session_dir = create_session_dir(&project_root, session_id);
    let review_text = r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Final fix review details.
<!-- CSA:SECTION:details:END -->

```toml findings.toml
[[findings]]
id = "final-fix"
severity = "high"
description = "Final fix reviewer output should replace stale findings."

[[findings.file_ranges]]
path = "tracked.txt"
start = 1
```"#;
    write_review_full_output(&session_dir, review_text);
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "[[findings]]\nid = \"stale\"\nseverity = \"low\"\ndescription = \"stale\"\n\n[[findings.file_ranges]]\npath = \"stale.txt\"\nstart = 9\n",
    )
    .expect("write stale findings artifact");

    let review_meta = make_review_meta(session_id);
    persist_fix_final_artifacts(&project_root, &review_meta);

    assert!(
        session_dir.join("review_meta.json").exists(),
        "expected review_meta.json to exist"
    );
    assert!(
        session_dir
            .join("output")
            .join("review-verdict.json")
            .exists(),
        "expected review-verdict.json to exist"
    );

    let findings_path = session_dir.join("output").join("findings.toml");
    assert!(findings_path.exists(), "expected findings.toml to exist");

    let findings_text = fs::read_to_string(&findings_path).expect("read findings.toml");
    let findings: FindingsFile = toml::from_str(&findings_text).expect("parse findings.toml");
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].id, "final-fix");
    assert_eq!(
        findings.findings[0].description,
        "Final fix reviewer output should replace stale findings."
    );
    assert_eq!(findings.findings[0].file_ranges[0].path, "tracked.txt");
    assert!(!findings_text.contains("id = \"stale\""));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
