use std::collections::{BTreeMap, HashMap};

use csa_process::StreamMode;
use csa_resource::isolation_plan::{EnforcementMode, IsolationPlanBuilder};

use crate::command_isolation::{
    CleanCommandContract as Contract, CleanRoomCapability as Capability,
};
use crate::transport::LegacyTransport as Legacy;
use crate::{
    assert_clean_request_rejected as reject, assert_invalid_clean_program as invalid_program,
    clean_test_claude as claude, clean_test_codex as codex, clean_test_contract as contract,
    clean_test_opencode as opencode, clean_test_script as test_script,
    clean_test_unsupported_executors as unsupported_executors, execute_clean_test,
};

use super::*;

fn options() -> ExecuteOptions {
    ExecuteOptions::new(StreamMode::BufferOnly, 60)
}

fn assert_prompt(executor: &Executor, prompt: &str) {
    let (command, stdin) = executor
        .build_clean_command(prompt, None, &contract("/bin/echo"))
        .unwrap();
    assert!(prompt.len() > MAX_ARGV_PROMPT_LEN || stdin.is_none());
    assert!(
        stdin.as_deref() == Some(prompt.as_bytes())
            || command
                .as_std()
                .get_args()
                .any(|argument| argument == prompt)
    );
}

#[test]
fn clean_contract_rejects_invalid_workdirs_before_spawn() {
    for program in ["relative", ""] {
        invalid_program(program);
    }
    let temp = tempfile::tempdir().unwrap();
    let (script, marker) = test_script(temp.path());
    let file = temp.path().join("file");
    std::fs::write(&file, "not a directory").unwrap();
    let invalid = vec![
        std::path::PathBuf::new(),
        "relative".into(),
        temp.path().join("missing"),
        file,
    ];
    for directory in invalid {
        assert!(Contract::try_new(&script, directory, BTreeMap::new()).is_err());
        assert!(!marker.exists());
    }
}

#[test]
fn clean_room_capability_is_explicit_for_every_legacy_executor() {
    assert!([codex(), opencode()].into_iter().all(|executor| matches!(
        Legacy::new(executor).clean_room_capability(),
        Capability::ExactPromptAndClearedEnvironment
    )));
    assert!(unsupported_executors().into_iter().all(|executor| matches!(
        Legacy::new(executor).clean_room_capability(),
        Capability::Unsupported { .. }
    )));
}

#[test]
fn clean_builder_preserves_short_and_long_prompts_for_supported_tools() {
    let short = "  $HOME 'quoted' \\slashes\\\nsecond line  ";
    let long = format!("  {short}{}  \n", "x".repeat(MAX_ARGV_PROMPT_LEN));
    for executor in [codex(), opencode()] {
        assert_prompt(&executor, short);
        assert_prompt(&executor, &long);
    }
}

#[test]
fn clean_claude_builder_proves_prompt_path_omits_legacy_preamble() {
    let prompt = " exact claude prompt ";
    let (command, stdin) = claude()
        .build_clean_command(prompt, None, &contract("/bin/echo"))
        .unwrap();
    assert!(stdin.is_none());
    assert!(
        command
            .as_std()
            .get_args()
            .any(|argument| argument == prompt)
    );
    assert!(
        !command
            .as_std()
            .get_args()
            .any(|argument| argument.to_string_lossy().contains("csa-sub-agent-context"))
    );
}

#[test]
fn clean_request_rejects_all_dual_authority_channels() {
    let base = options();
    reject(Some(&HashMap::new()), &base);

    let mut options = base.clone();
    options.allow_git_push = true;
    reject(None, &options);

    let mut options = base.clone();
    options.subtree_pin = csa_core::env::SubtreeModelPin::from_validated_spec("codex/x", true);
    reject(None, &options);

    let mut options = base;
    options.sandbox = Some(SandboxContext {
        isolation_plan: IsolationPlanBuilder::new(EnforcementMode::Off)
            .build()
            .unwrap(),
        tool_name: "opencode".into(),
        session_id: "clean".into(),
        best_effort: true,
    });
    reject(None, &options);
}

#[tokio::test]
async fn supported_direct_path_spawns_once_and_delivers_exact_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let (script, marker) = test_script(cwd);
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'x\\n' >> \"$MARKER\"\npwd\nlast=\nfor arg in \"$@\"; do last=$arg; done\nprintf '%s' \"$last\"\n",
    )
    .unwrap();

    let prompt = " prompt ";
    let clean = Contract::try_new(
        script,
        cwd,
        BTreeMap::from([("MARKER".into(), marker.display().to_string())]),
    )
    .unwrap();
    let session = crate::transport::build_ephemeral_meta_session(cwd);
    let parent_cwd = std::env::current_dir().unwrap();
    assert_ne!(parent_cwd, cwd);
    let result = execute_clean_test(&opencode(), prompt, &session, options(), clean)
        .await
        .unwrap();
    assert_eq!(
        result.execution.output,
        format!("{}\n{prompt}", cwd.display())
    );
    assert_eq!(std::env::current_dir().unwrap(), parent_cwd);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "x\n");
}

#[tokio::test]
async fn unsupported_transport_and_pre_session_hook_fail_before_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let session = crate::transport::build_ephemeral_meta_session(temp.path());
    let hook = csa_hooks::PreSessionHookInvocation::new(csa_hooks::PreSessionHookConfig {
        command: Some("must-not-run".into()),
        ..Default::default()
    });
    for (executor, options, expected) in [
        (claude(), options(), "unsupported"),
        (
            opencode(),
            options().with_pre_session_hook(hook),
            "pre-session",
        ),
    ] {
        let error = execute_clean_test(
            &executor,
            "prompt",
            &session,
            options,
            contract("/definitely/missing"),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }
}
