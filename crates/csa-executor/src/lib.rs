//! Executor enum for AI tools with unified model spec.

pub mod agent_backend_adapter;
pub mod codex_runtime;
pub mod context_loader;
pub mod design_context;
pub mod executor;
pub mod install_hints;
pub mod logging;
pub mod model_spec;
pub mod session_id;
pub mod transport;
pub(crate) mod transport_gemini_retry;
pub mod transport_openai_compat;

pub use agent_backend_adapter::ExecutorAgentBackend;
pub use codex_runtime::{CodexRuntimeMetadata, CodexTransport, codex_runtime_metadata};
pub use context_loader::{
    ContextFile, ContextLoadOptions, format_context_for_prompt, load_project_context,
    structured_output_instructions, structured_output_instructions_for_fork_call,
};
pub use csa_process::ExecutionResult;
pub use design_context::{extract_design_sections, format_design_context};
pub use executor::{ExecuteOptions, Executor, SandboxContext};
pub use install_hints::{
    CLAUDE_CODE_ACP_INSTALL_HINT, GEMINI_CLI_INSTALL_HINT, OPENAI_COMPAT_INSTALL_HINT,
    OPENCODE_INSTALL_HINT, install_hint_for_known_tool,
};
pub use logging::create_session_log_writer;
pub use model_spec::{ModelSpec, ThinkingBudget};
pub use session_id::{extract_session_id, extract_session_id_from_transport};
pub use transport::{
    AcpTransport, LegacyTransport, PeakMemoryContext, SandboxTransportConfig, Transport,
    TransportFactory, TransportMode, TransportOptions, TransportResult,
};

// Re-export session config types from csa-acp for pipeline integration.
pub use csa_acp::{McpServerConfig as AcpMcpServerConfig, SessionConfig};
