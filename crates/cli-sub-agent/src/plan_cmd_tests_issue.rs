use super::*;
use std::collections::HashMap;
use weave::compiler::{FailAction, PlanStep};

#[tokio::test]
async fn execute_step_bash_receives_issue_number_variable() {
    let step = PlanStep {
        id: 1,
        title: "issue env".into(),
        tool: Some("bash".into()),
        prompt: "```bash\nprintf '%s' \"${ISSUE_NUMBER}\" > issue_number.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::from([(ISSUE_NUMBER_VAR.to_string(), "1663".to_string())]);
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("issue_number.txt")).unwrap(),
        "1663"
    );
}
