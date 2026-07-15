//! Executor enum for AI tools with unified model spec.

pub mod agent_backend_adapter;
pub mod claude_runtime;
pub mod codex_runtime;
pub mod command_isolation;
pub mod context_loader;
pub mod design_context;
pub mod executor;
#[cfg(feature = "acp")]
pub mod hermes_config;
pub mod install_hints;
mod lefthook_guard;
pub mod logging;
pub mod model_spec;
pub mod session_config;
pub mod session_id;
pub mod transport;
pub(crate) mod transport_gemini_oauth;
pub(crate) mod transport_gemini_retry;
pub mod transport_openai_compat;
pub mod transport_tmux;

pub use agent_backend_adapter::ExecutorAgentBackend;
pub use claude_runtime::{ClaudeCodeRuntimeMetadata, ClaudeCodeTransport, claude_runtime_metadata};
pub use codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};
pub use context_loader::{
    ContextFile, ContextLoadOptions, format_context_for_prompt, load_project_context,
    structured_output_instructions, structured_output_instructions_for_fork_call,
};
pub use csa_process::ExecutionResult;
pub use design_context::{extract_design_sections, format_design_context};
pub use executor::executor_env::STRIPPED_ENV_VARS as CHILD_PROCESS_STRIPPED_ENV_VARS;
pub use executor::{ExecuteOptions, Executor, SandboxContext};
#[cfg(feature = "acp")]
pub use hermes_config::HermesRunConfig;
pub use install_hints::{
    CLAUDE_CODE_ACP_INSTALL_HINT, CLAUDE_CODE_CLI_INSTALL_HINT, GEMINI_CLI_INSTALL_HINT,
    HERMES_INSTALL_HINT, OPENAI_COMPAT_INSTALL_HINT, OPENCODE_INSTALL_HINT,
    install_hint_for_known_tool,
};
pub use logging::create_session_log_writer;
pub use model_spec::{ModelSpec, ThinkingBudget};
pub use session_config::{
    McpServerConfig as AcpMcpServerConfig, SessionConfig, ToolOutputCompactionConfig,
};
pub use session_id::{extract_session_id, extract_session_id_from_transport};
#[cfg(feature = "acp")]
pub use transport::AcpTransport;
pub use transport::{
    CODEX_EXEC_INITIAL_STALL_REASON, ClaudeCodeCliTransport,
    DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS, GEMINI_OAUTH_PROMPT_FATAL_MARKER,
    LegacyTransport, PeakMemoryContext, ResolvedTimeout, SandboxTransportConfig, Transport,
    TransportCapabilities, TransportFactory, TransportFactoryError, TransportMode,
    TransportOptions, TransportResult, apply_codex_exec_initial_stall_summary,
    classify_codex_exec_initial_stall, contains_gemini_oauth_prompt, normalize_gemini_prompt_text,
    resolve_initial_response_timeout, strip_ansi_escape_sequences,
};
pub use transport_tmux::{TmuxReapStats, TmuxTransport, reap_orphan_tmux_sessions};

#[cfg(test)]
fn clean_test_codex() -> Executor {
    Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: codex_runtime_metadata(),
    }
}

#[cfg(test)]
fn clean_test_opencode() -> Executor {
    Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    }
}

#[cfg(test)]
fn clean_test_claude() -> Executor {
    Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: claude_runtime_metadata(),
    }
}

#[cfg(test)]
fn clean_test_unsupported_executors() -> [Executor; 5] {
    [
        clean_test_claude(),
        Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        },
        Executor::OpenaiCompat {
            model_override: None,
            thinking_budget: None,
        },
        Executor::Hermes {
            provider_override: None,
            model_override: None,
            thinking_budget: None,
        },
        Executor::AntigravityCli {
            model_override: None,
            thinking_budget: None,
        },
    ]
}

#[cfg(all(test, unix))]
fn clean_test_script(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let script = root.join("fake-opencode");
    let marker = root.join("spawn-count");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'x\\n' >> \"$MARKER\"\nlast=\nfor arg in \"$@\"; do last=$arg; done\nprintf '%s' \"$last\"\n",
    )
    .expect("write fake");
    let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make executable");
    (script, marker)
}

#[cfg(test)]
fn clean_test_contract(
    program: impl Into<std::path::PathBuf>,
) -> command_isolation::CleanCommandContract {
    command_isolation::CleanCommandContract::try_new(
        program,
        env!("CARGO_MANIFEST_DIR"),
        std::collections::BTreeMap::new(),
    )
    .expect("valid contract")
}

#[cfg(test)]
async fn execute_clean_test(
    executor: &Executor,
    prompt: &str,
    session: &csa_session::state::MetaSessionState,
    options: ExecuteOptions,
    contract: command_isolation::CleanCommandContract,
) -> anyhow::Result<TransportResult> {
    executor
        .execute_with_command_isolation(
            prompt,
            None,
            session,
            None,
            options,
            None,
            command_isolation::CommandIsolationPolicy::CleanRoom(contract),
        )
        .await
}

#[cfg(test)]
fn assert_clean_request_rejected(
    extra_env: Option<&std::collections::HashMap<String, String>>,
    options: &ExecuteOptions,
) {
    assert!(executor::validate_clean_room_request(extra_env, options).is_err());
}

#[cfg(test)]
fn assert_invalid_clean_program(program: &str) {
    assert!(
        command_isolation::CleanCommandContract::try_new(
            program,
            env!("CARGO_MANIFEST_DIR"),
            std::collections::BTreeMap::new()
        )
        .is_err()
    );
}
