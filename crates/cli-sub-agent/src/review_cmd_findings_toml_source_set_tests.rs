use super::extract_findings_toml_from_text;
use csa_session::{FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity};

#[test]
fn extract_findings_toml_from_text_merges_multiple_labeled_blocks() {
    let review_text = r#"```findings.toml
findings = []
```

```toml findings.toml
[[findings]]
id = "f1"
severity = "high"
description = "Later non-empty labeled block must not be hidden."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 154
```

```findings.toml
[[findings]]
id = "f1"
severity = "high"
description = "Later non-empty labeled block must not be hidden."

[[findings.file_ranges]]
path = "crates/cli-sub-agent/src/review_cmd_findings_toml.rs"
start = 154
```
"#;

    let parsed =
        extract_findings_toml_from_text(review_text).expect("findings.toml block should parse");

    assert_eq!(
        parsed,
        FindingsFile {
            findings: vec![ReviewFinding {
                id: "f1".to_string(),
                severity: Severity::High,
                file_ranges: vec![ReviewFindingFileRange {
                    path: "crates/cli-sub-agent/src/review_cmd_findings_toml.rs".to_string(),
                    start: 154,
                    end: None,
                }],
                is_regression_of_commit: None,
                suggested_test_scenario: None,
                description: "Later non-empty labeled block must not be hidden.".to_string(),
            }],
        }
    );
}
