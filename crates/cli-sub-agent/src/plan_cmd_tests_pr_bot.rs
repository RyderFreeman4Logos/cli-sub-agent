use super::plan_cmd_steps::execute_step_with_workflow;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use weave::compiler::{FailAction, PlanStep, plan_from_toml};

#[tokio::test]
async fn execute_step_with_workflow_exposes_runtime_paths_to_bash() {
    let project_root = tempfile::tempdir().unwrap();
    let workflow_home = tempfile::tempdir().unwrap();
    let workflow_path = workflow_home.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='runtime-env'\n").unwrap();

    let step = PlanStep {
        id: 1,
        title: "runtime env".into(),
        tool: Some("bash".into()),
        prompt: "```bash\nprintf '%s\\n%s\\n%s\\n' \"$CSA_PROJECT_ROOT\" \"$CSA_WORKFLOW_PATH\" \"$CSA_WORKFLOW_DIR\" > runtime-env.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();

    let result = execute_step_with_workflow(
        &step,
        &vars,
        project_root.path(),
        &workflow_path,
        None,
        None,
    )
    .await;
    assert_eq!(result.exit_code, 0, "bash step should succeed");

    let env_dump = std::fs::read_to_string(project_root.path().join("runtime-env.txt")).unwrap();
    let mut lines = env_dump.lines();
    assert_eq!(
        Path::new(lines.next().expect("missing project root env")),
        project_root.path()
    );
    assert_eq!(
        Path::new(lines.next().expect("missing workflow path env")),
        workflow_path.as_path()
    );
    assert_eq!(
        Path::new(lines.next().expect("missing workflow dir env")),
        workflow_home.path()
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn git_archive_entries(repo_root: &Path, pathspec: &str) -> Vec<String> {
    let tree = Command::new("git")
        .args(["write-tree"])
        .current_dir(repo_root)
        .output()
        .expect("git write-tree should run");
    assert!(
        tree.status.success(),
        "git write-tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree_id = String::from_utf8(tree.stdout)
        .expect("tree id should be utf-8")
        .trim()
        .to_string();

    let archive = Command::new("git")
        .args(["archive", "--format=tar", &tree_id, pathspec])
        .current_dir(repo_root)
        .output()
        .expect("git archive should run");
    assert!(
        archive.status.success(),
        "git archive failed: {}",
        String::from_utf8_lossy(&archive.stderr)
    );

    let mut tar = Command::new("tar")
        .args(["tf", "-"])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("tar should start");
    tar.stdin
        .as_mut()
        .expect("tar stdin")
        .write_all(&archive.stdout)
        .expect("should stream archive into tar");
    let listing = tar.wait_with_output().expect("tar should finish");
    assert!(
        listing.status.success(),
        "tar listing failed: {}",
        String::from_utf8_lossy(&listing.stderr)
    );
    String::from_utf8(listing.stdout)
        .expect("tar output should be utf-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn pr_bot_workflow_is_v1_loop_free() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let loop_steps: Vec<usize> = plan
        .steps
        .iter()
        .filter_map(|step| step.loop_var.as_ref().map(|_| step.id))
        .collect();

    assert!(
        loop_steps.is_empty(),
        "pr-bot must remain v1-compatible; loop_var found on steps {loop_steps:?}"
    );
}

#[test]
fn pr_bot_workflow_resolves_helpers_from_pattern_dir() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();

    assert!(
        workflow.contains("CSA_HELPER_DIR=\"${CSA_WORKFLOW_DIR}/scripts/csa\""),
        "pr-bot must resolve bundled helpers from the workflow directory"
    );
    assert!(
        !workflow.contains("bash scripts/csa/"),
        "pr-bot must not depend on the target repo's scripts/ directory"
    );
}

#[test]
fn pr_bot_archive_includes_helper_scripts() {
    let entries = git_archive_entries(&workspace_root(), "patterns/pr-bot");

    assert!(
        entries.contains(&"patterns/pr-bot/scripts/csa/latest-pass-review-head.sh".to_string()),
        "git archive for patterns/pr-bot must include latest-pass-review-head.sh"
    );
    assert!(
        entries.contains(&"patterns/pr-bot/scripts/csa/session-wait-until-done.sh".to_string()),
        "git archive for patterns/pr-bot must include session-wait-until-done.sh"
    );
}
