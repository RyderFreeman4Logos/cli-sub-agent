use std::fs;

use csa_core::types::ReviewDecision;
use csa_session::FindingsFile;
use csa_session::state::ReviewSessionMeta;

use super::{FINDINGS_TOML_SYNTHETIC_MARKER, persist_review_findings_toml};

fn review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    }
}

#[test]
fn issue_1953_explicit_empty_findings_toml_beats_prior_round_recheck_prose() {
    let project_root = tempfile::tempdir().expect("temp project root");
    let _state_home = crate::test_env_lock::ScopedTestEnvVar::set(
        "XDG_STATE_HOME",
        project_root.path().join("state"),
    );
    let session_id = "01TEST1953PASSRECHECK000";
    let session_dir =
        csa_session::get_session_dir(project_root.path(), session_id).expect("session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    let review_text = r#"Reviewer progress: prior blocking defect is resolved.

<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Scope reviewed: `range:main...HEAD`.

No blocking findings found.

Prior-round recheck:
- `src/mcp/server.rs:3952` novelty merge/audit ordering is resolved.

Open questions: none.
<!-- CSA:SECTION:details:END -->

```findings.toml
findings = []
```"#;
    let full_output = serde_json::to_string(&serde_json::json!({
        "type": "item.completed",
        "item": {
            "type": "agent_message",
            "text": review_text
        }
    }))
    .expect("serialize transcript");
    fs::write(session_dir.join("output").join("full.md"), full_output).expect("write full output");
    csa_session::persist_structured_output(&session_dir, review_text)
        .expect("persist structured output");

    persist_review_findings_toml(project_root.path(), &review_meta(session_id));

    let actual = fs::read_to_string(session_dir.join("output").join("findings.toml"))
        .expect("read findings.toml");
    let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());
    assert_eq!(actual.trim(), "findings = []");
    assert!(
        !session_dir
            .join("output")
            .join(FINDINGS_TOML_SYNTHETIC_MARKER)
            .exists(),
        "explicit empty findings.toml must remain authoritative, not synthetic"
    );
}
