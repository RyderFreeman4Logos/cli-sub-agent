use super::super::*;
use std::collections::HashMap;
use weave::compiler::{FailAction, PlanStep};

#[tokio::test]
async fn execute_step_failure_reports_stderr_tail() {
    let step = PlanStep {
        id: 1,
        title: "stderr failure".into(),
        tool: Some("bash".into()),
        prompt:
            "```bash\nfor i in $(seq 1 25); do printf 'err-%02d\\n' \"$i\" >&2; done\nexit 1\n```"
                .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_eq!(result.exit_code, 1);
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Exit code 1"),
        "failure summary must keep exit code: {error}"
    );
    assert!(
        error.contains("stderr (last 20 lines):"),
        "failure summary must label stderr tail: {error}"
    );
    assert!(
        error.contains("err-25"),
        "failure summary must include final stderr line: {error}"
    );
    assert!(
        !error.contains("err-01"),
        "failure summary must keep only the stderr tail: {error}"
    );
    let command = result.command.as_deref().unwrap_or_default();
    assert!(
        command.contains("exit 1"),
        "failure report command must include the executed bash script: {command}"
    );
    let stderr = result.stderr.as_deref().unwrap_or_default();
    assert!(
        stderr.contains("err-25") && !stderr.contains("err-01"),
        "structured stderr excerpt must keep the same tail as the error summary: {stderr}"
    );
}
