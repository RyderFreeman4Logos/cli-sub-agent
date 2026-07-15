use std::collections::{BTreeMap, HashMap};

use csa_process::StreamMode;
use csa_resource::isolation_plan::IsolationPlanBuilder;

use crate::command_isolation::{CleanCommandContract, CleanRoomCapability};
use crate::transport::LegacyTransport;
use crate::{
    assert_clean_request_rejected as reject, assert_invalid_clean_program as invalid_program,
    clean_test_claude as claude, clean_test_codex as codex, clean_test_contract as contract,
    clean_test_opencode as opencode, clean_test_script as test_script,
    clean_test_unsupported_executors as unsupported_executors, execute_clean_test,
};

use super::*;

#[test]
fn clean_contract_requires_an_absolute_program() {
    for program in ["relative", ""] {
        invalid_program(program);
    }
}

#[test]
fn clean_room_capability_is_explicit_for_every_legacy_executor() {
    for executor in [codex(), opencode()] {
        assert_eq!(
            LegacyTransport::new(executor).clean_room_capability(),
            CleanRoomCapability::ExactPromptAndClearedEnvironment
        );
    }
    for executor in unsupported_executors() {
        assert!(matches!(
            LegacyTransport::new(executor).clean_room_capability(),
            CleanRoomCapability::Unsupported { .. }
        ));
    }
}

#[test]
fn clean_builder_preserves_short_and_long_prompts_for_supported_tools() {
    let short = "  $HOME 'quoted' \\slashes\\\nsecond line  ";
    let long = format!("  {short}{}  \n", "x".repeat(MAX_ARGV_PROMPT_LEN));
    for executor in [codex(), opencode()] {
        let clean = contract("/bin/echo");
        let (short_cmd, short_stdin) = executor
            .build_clean_command(short, None, &clean)
            .expect("short command");
        assert!(short_stdin.is_none());
        assert!(short_cmd.as_std().get_args().any(|arg| arg == short));

        let (long_cmd, long_stdin) = executor
            .build_clean_command(&long, None, &clean)
            .expect("long command");
        if let Some(stdin) = long_stdin {
            assert!(stdin.as_slice() == long.as_bytes());
        } else {
            assert!(long_cmd.as_std().get_args().any(|arg| arg == long.as_str()));
        }
    }
}

#[test]
fn clean_claude_builder_proves_prompt_path_omits_legacy_preamble() {
    let prompt = " exact claude prompt ";
    let (cmd, stdin) = claude()
        .build_clean_command(prompt, None, &contract("/bin/echo"))
        .expect("clean command");
    assert!(stdin.is_none());
    let args = cmd
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.contains(&prompt.to_string()));
    assert!(!args.iter().any(|arg| arg.contains("csa-sub-agent-context")));
}

#[test]
fn clean_request_rejects_all_dual_authority_channels() {
    let base = ExecuteOptions::new(StreamMode::BufferOnly, 60);
    let extra_env = HashMap::new();
    reject(Some(&extra_env), &base);

    let mut options = base.clone();
    options.pre_session_hook = Some(csa_hooks::PreSessionHookInvocation::new(
        csa_hooks::PreSessionHookConfig {
            command: Some("must-not-run".into()),
            ..Default::default()
        },
    ));
    reject(None, &options);

    let mut options = base.clone();
    options.allow_git_push = true;
    reject(None, &options);

    let mut options = base.clone();
    options.subtree_pin = csa_core::env::SubtreeModelPin::from_validated_spec("codex/x", true);
    reject(None, &options);

    let mut options = base;
    options.sandbox = Some(SandboxContext {
        isolation_plan: IsolationPlanBuilder::new(
            csa_resource::isolation_plan::EnforcementMode::Off,
        )
        .build()
        .expect("plan"),
        tool_name: "opencode".into(),
        session_id: "clean".into(),
        best_effort: true,
    });
    reject(None, &options);
}

#[tokio::test]
async fn supported_direct_path_spawns_once_and_delivers_exact_prompt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (script, marker) = test_script(temp.path());

    let prompt = "  exact $HOME 'prompt' \\ value  ";
    let clean = CleanCommandContract::try_new(
        script,
        BTreeMap::from([("MARKER".to_string(), marker.display().to_string())]),
    )
    .expect("contract");
    let session = crate::transport::build_ephemeral_meta_session(temp.path());
    let result = execute_clean_test(
        &opencode(),
        prompt,
        &session,
        ExecuteOptions::new(StreamMode::BufferOnly, 60),
        clean,
    )
    .await
    .expect("clean execution");
    assert_eq!(result.execution.output, prompt);
    assert_eq!(
        std::fs::read_to_string(marker).expect("spawn marker"),
        "x\n",
        "clean direct transport must spawn exactly once"
    );
}

#[tokio::test]
async fn unsupported_transport_and_pre_session_hook_fail_before_spawn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = crate::transport::build_ephemeral_meta_session(temp.path());
    let unsupported = execute_clean_test(
        &claude(),
        "prompt",
        &session,
        ExecuteOptions::new(StreamMode::BufferOnly, 60),
        contract("/definitely/missing"),
    )
    .await
    .expect_err("configured ACP transport is unsupported");
    assert!(unsupported.to_string().contains("unsupported"));

    let hook = csa_hooks::PreSessionHookInvocation::new(csa_hooks::PreSessionHookConfig {
        command: Some("must-not-run".into()),
        ..Default::default()
    });
    let rejected = execute_clean_test(
        &opencode(),
        "prompt",
        &session,
        ExecuteOptions::new(StreamMode::BufferOnly, 60).with_pre_session_hook(hook),
        contract("/definitely/missing"),
    )
    .await
    .expect_err("hook must fail before transport invocation");
    assert!(rejected.to_string().contains("pre-session"));
}
