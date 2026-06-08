use super::*;

#[test]
fn serialize_step_result_json_redacts_failure_metadata() {
    let result = StepResult {
        step_id: 1,
        title: "secret failure".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 1\nstderr:\npassword=hunter2".to_string()),
        output: None,
        session_id: None,
        command: Some(
            "curl -H 'Authorization: Bearer abcDEF123._-token' api_key=key-prod_987654321"
                .to_string(),
        ),
        stderr: Some("client_secret=top-secret-value".to_string()),
    };

    let json = serialize_step_result_json(&result);

    assert!(
        json.contains("[REDACTED]"),
        "redacted JSON must mark secrets: {json}"
    );
    assert!(
        !json.contains("abcDEF123._-token"),
        "bearer token leaked: {json}"
    );
    assert!(
        !json.contains("key-prod_987654321"),
        "api key leaked: {json}"
    );
    assert!(!json.contains("hunter2"), "password leaked: {json}");
    assert!(
        !json.contains("top-secret-value"),
        "client secret leaked: {json}"
    );
    serde_json::from_str::<serde_json::Value>(&json).expect("redacted chunk must stay JSON");
}
