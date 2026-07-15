use std::collections::BTreeMap;

use csa_core::types::ToolName;

use super::clean_room_execution_tests::{clean_limits, command_contract};
use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;

fn low_resource_config() -> csa_config::ProjectConfig {
    toml::from_str(
        r#"
[resources]
min_free_memory_mb = 1
idle_timeout_seconds = 30
initial_response_timeout_seconds = 10
"#,
    )
    .expect("clean-room test config")
}

#[cfg(unix)]
#[tokio::test]
async fn clean_room_executes_admitted_fake_and_leaves_only_minimal_session_artifacts() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new(&temp).await;
    sandbox.track_env(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV);
    sandbox.track_env("CSA_CLEAN_ROOM_PARENT_SENTINEL");
    unsafe {
        std::env::set_var(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
        std::env::set_var("CSA_CLEAN_ROOM_PARENT_SENTINEL", "must-not-leak");
    }

    let project = temp.path().join("workspace");
    let clean_home = project.join("home");
    let evidence = temp.path().join("evidence.md");
    let program = project.join("fake-opencode");
    std::fs::create_dir_all(&clean_home).expect("clean home");
    std::fs::write(project.join("source.txt"), "immutable source\n").expect("source");
    std::fs::write(&evidence, "frozen evidence\n").expect("evidence");
    std::fs::write(
        &program,
        r#"#!/bin/sh
set -eu
[ "${CSA_CLEAN_ROOM_PARENT_SENTINEL+x}" != x ]
[ "${ONLY_EXPLICIT}" = allowed ]
[ "$(pwd)" = "${EXPECTED_CWD}" ]
last=
for arg in "$@"; do last=$arg; done
printf '%s' "$last"
"#,
    )
    .expect("fake command");
    let mut permissions = std::fs::metadata(&program).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("make executable");

    let admitted = build_and_validate_executor(
        &ToolName::Opencode,
        Some("opencode/openai/gpt-5/xhigh"),
        None,
        None,
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: None,
        },
        false,
        false,
        false,
    )
    .await
    .expect("catalog-admitted fake executor");
    let config = low_resource_config();
    let global = csa_config::GlobalConfig::default();
    let prompt =
        "\u{feff}artifact précis\r\n<csa-sub-agent-context>not-a-guard</csa-sub-agent-context>\r\n";
    let explicit_environment = BTreeMap::from([
        ("HOME".to_string(), clean_home.display().to_string()),
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("ONLY_EXPLICIT".to_string(), "allowed".to_string()),
        ("EXPECTED_CWD".to_string(), project.display().to_string()),
    ]);
    let source_before = std::fs::read(project.join("source.txt")).expect("source before");

    let mut ids = Vec::new();
    for _ in 0..2 {
        let contract = CleanRoomExecutionContract::try_new(
            &project,
            &evidence,
            command_contract(&program, &project, explicit_environment.clone()),
        )
        .expect("validated pipeline clean-room contract");
        let outcome = execute_clean_room_session(
            &admitted,
            &ToolName::Opencode,
            prompt,
            contract,
            Some(&config),
            Some(&global),
            clean_limits(),
        )
        .await
        .expect("fake clean-room execution");

        assert_eq!(
            outcome.execution.exit_code, 0,
            "execution={:#?}",
            outcome.execution
        );
        assert_eq!(outcome.execution.output.as_bytes(), prompt.as_bytes());
        assert!(outcome.provider_session_id.is_none());
        let session = csa_session::load_session(&project, &outcome.meta_session_id)
            .expect("load clean session");
        assert!(session.genealogy.parent_session_id.is_none());
        assert!(
            session.tools.is_empty(),
            "clean completion must not persist tool state"
        );
        let session_dir =
            csa_session::get_session_dir(&project, &outcome.meta_session_id).expect("session dir");
        assert!(session_dir.join("result.toml").is_file());
        for forbidden in [
            "output.log",
            "handoff.toml",
            "input/effective_prompt.txt",
            "output/transcript.jsonl",
            ".cooldown",
        ] {
            assert!(
                !session_dir.join(forbidden).exists(),
                "forbidden clean-room artifact exists: {forbidden}"
            );
        }
        let persisted = csa_session::load_result(&project, &outcome.meta_session_id)
            .expect("load result")
            .expect("minimal result");
        assert_eq!(persisted.exit_code, 0);
        assert!(persisted.artifacts.is_empty());
        ids.push(outcome.meta_session_id);
    }

    assert_ne!(ids[0], ids[1], "every clean-room execution must be fresh");
    assert_eq!(
        std::fs::read(project.join("source.txt")).expect("source after"),
        source_before
    );
    assert!(!project.join("hook-fired").exists());
}

#[tokio::test]
async fn clean_room_execution_policy_rejects_admitted_identity_mismatch_before_session_creation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new(&temp).await;
    sandbox.track_env(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV);
    unsafe {
        std::env::set_var(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    }
    let project = temp.path().join("workspace");
    let evidence = temp.path().join("evidence.md");
    let program = project.join("never-run");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(&evidence, "evidence").expect("evidence");
    std::fs::write(&program, "#!/bin/sh\nexit 97\n").expect("program");
    let admitted = build_and_validate_executor(
        &ToolName::Opencode,
        Some("opencode/openai/gpt-5/xhigh"),
        None,
        None,
        ConfigRefs {
            project: None,
            global: None,
            model_catalog: None,
        },
        false,
        false,
        false,
    )
    .await
    .expect("admitted");
    let contract = CleanRoomExecutionContract::try_new(
        &project,
        &evidence,
        command_contract(&program, &project, BTreeMap::new()),
    )
    .expect("contract");

    let error = execute_clean_room_session(
        &admitted,
        &ToolName::Codex,
        "must not execute",
        contract,
        Some(&low_resource_config()),
        Some(&csa_config::GlobalConfig::default()),
        clean_limits(),
    )
    .await
    .expect_err("identity mismatch must fail");
    assert!(error.to_string().contains("admitted"));
    let session_root = csa_session::get_session_root(&project).expect("session root");
    assert!(
        !session_root.join("sessions").exists(),
        "identity mismatch must fail before session bootstrap"
    );
}
