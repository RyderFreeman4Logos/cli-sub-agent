//! Executor enum for 4 AI tools with unified model spec.

pub mod agent_backend_adapter;
pub mod context_loader;
pub mod executor;
pub mod logging;
pub mod model_spec;
pub mod session_id;
pub mod transport;

pub use agent_backend_adapter::ExecutorAgentBackend;
pub use context_loader::{
    ContextFile, ContextLoadOptions, format_context_for_prompt, load_project_context,
};
pub use csa_process::ExecutionResult;
pub use executor::Executor;
pub use logging::create_session_log_writer;
pub use model_spec::{ModelSpec, ThinkingBudget};
pub use session_id::{extract_session_id, extract_session_id_from_transport};
pub use transport::{
    AcpTransport, LegacyTransport, Transport, TransportFactory, TransportMode, TransportResult,
};
