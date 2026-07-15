use std::collections::{BTreeMap as Map, HashMap};

use csa_resource::filesystem_sandbox::FilesystemCapability as FsCapability;
use csa_resource::isolation_plan::{EnforcementMode as Mode, IsolationPlanBuilder as PlanBuilder};
use tokio::process::Command;

use super::{
    ClearedCommandEnvironment as CleanEnvironment,
    spawn_tool_sandboxed_in_environment as spawn_clean, *,
};

async fn run_clean(
    command: Command,
    stdin: Option<Vec<u8>>,
    session_id: &str,
    environment: CleanEnvironment,
) -> ExecutionResult {
    let (child, _) = spawn_clean(
        command,
        stdin,
        SpawnOptions::default(),
        None,
        "test",
        session_id,
        &environment,
    )
    .await
    .expect("spawn clean child");
    wait_and_capture(child, StreamMode::BufferOnly)
        .await
        .expect("capture child")
}

#[test]
fn clean_environment_rejects_invalid_and_reserved_entries() {
    for (key, value) in [
        ("", "value"),
        ("BAD=KEY", "value"),
        ("BAD\0KEY", "value"),
        ("BAD_VALUE", "bad\0value"),
        (csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY, "true"),
        (csa_core::env::CSA_MODEL_SPEC_ENV_KEY, "model"),
    ] {
        let error = CleanEnvironment::try_new(Map::from([(key.to_string(), value.to_string())]))
            .expect_err("entry must be rejected");
        assert!(
            error
                .to_string()
                .contains(key.split('\0').next().unwrap_or(""))
        );
    }
}

#[test]
fn isolation_environment_is_sorted_and_conflicts_fail_closed() {
    let mut plan = PlanBuilder::new(Mode::Off).build().expect("plan");
    plan.env_overrides = HashMap::from([
        ("PLAN_ONLY".into(), "plan".into()),
        ("SAME".into(), "value".into()),
    ]);
    let environment = clean_env(&[("CALLER_ONLY", "caller"), ("SAME", "value")]);
    let effective = environment.effective_entries(Some(&plan)).expect("merge");
    assert_eq!(
        effective.into_iter().collect::<Vec<_>>(),
        vec![
            ("CALLER_ONLY".into(), "caller".into()),
            ("PLAN_ONLY".into(), "plan".into()),
            ("SAME".into(), "value".into()),
        ]
    );

    plan.env_overrides.insert("SAME".into(), "different".into());
    let error = environment
        .effective_entries(Some(&plan))
        .expect_err("conflict must fail closed");
    assert!(error.to_string().contains("SAME"));
}

#[test]
fn bwrap_inner_home_and_outer_environment_come_from_the_typed_map() {
    let plan = PlanBuilder::new(Mode::Off)
        .with_filesystem_capability(FsCapability::Bwrap)
        .with_filesystem_enforcement(Mode::Required)
        .with_writable_path("/tmp".into())
        .build()
        .expect("plan");
    let environment = clean_env(&[("HOME", "/tmp/clean-home"), ("TOKEN", "secret")]);
    let effective = environment
        .effective_entries(Some(&plan))
        .expect("effective");
    let command = Command::new("/bin/true");
    let mut wrapped = spawn::wrap_command_with_bwrap_required(command, &plan, &effective)
        .expect("required wrapper");
    command_environment::apply_cleared_environment(wrapped.as_std_mut(), &effective);

    let args = wrapped
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        args.windows(3)
            .any(|args| args == ["--setenv", "HOME", "/tmp/clean-home"])
    );
    assert!(!args.iter().any(|arg| arg == "secret"));
    assert_recorded_env(&wrapped, &effective);
}

#[test]
fn cgroup_outer_environment_is_reasserted_from_the_typed_map() {
    let environment = clean_env(&[("PATH", "/usr/bin:/bin"), ("TOKEN", "secret")]);
    let effective = environment.effective_entries(None).expect("effective");
    let command = Command::new("/bin/true");
    let wrapped = spawn::build_clean_cgroup_scope_command(
        &command,
        "opencode",
        "clean",
        &csa_resource::cgroup::SandboxConfig {
            memory_max_mb: 512,
            memory_swap_max_mb: None,
            pids_max: Some(32),
        },
        &effective,
    )
    .expect("cgroup wrapper");
    assert_eq!(wrapped.as_std().get_program(), "systemd-run");
    assert!(wrapped.as_std().get_args().any(|arg| arg == "/bin/true"));
    assert_recorded_env(&wrapped, &effective);
}

#[tokio::test]
async fn final_spawn_clears_parent_and_command_environment() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "test -z \"${HOME+x}\" && test -z \"${COMMAND_SENTINEL+x}\" && printf '%s' \"$EXPLICIT_SENTINEL\"",
    ]);
    cmd.env("COMMAND_SENTINEL", "must-not-leak");
    let result = run_clean(
        cmd,
        None,
        "clean-env",
        clean_env(&[("EXPLICIT_SENTINEL", "present")]),
    )
    .await;
    assert_eq!(result.exit_code, 0, "{}", result.stderr_output);
    assert_eq!(result.output, "present");
}

#[tokio::test]
async fn stdin_prompt_bytes_are_delivered_exactly() {
    let prompt = b"  first line\n$HOME 'quoted' \\slashes\\\nlast line  \n".to_vec();
    let cmd = Command::new("/bin/cat");
    let result = run_clean(cmd, Some(prompt.clone()), "exact-stdin", clean_env(&[])).await;
    assert_eq!(result.output.as_bytes(), prompt);
}

#[tokio::test]
async fn relative_program_requires_an_explicit_path() {
    let cmd = Command::new("sh");
    let result = spawn_clean(
        cmd,
        None,
        SpawnOptions::default(),
        None,
        "test",
        "missing-path",
        &clean_env(&[]),
    )
    .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("relative program must fail before spawn"),
    };
    assert!(error.to_string().contains("explicit PATH"));
}
