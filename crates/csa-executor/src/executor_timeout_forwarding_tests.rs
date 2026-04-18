use super::*;
use crate::codex_runtime::{CodexRuntimeMetadata, CodexTransport};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct RecordingTransport {
    captured_timeouts: Arc<Mutex<Vec<Option<u64>>>>,
}

#[async_trait]
impl Transport for RecordingTransport {
    fn mode(&self) -> crate::transport::TransportMode {
        crate::transport::TransportMode::Legacy
    }

    async fn execute(
        &self,
        _prompt: &str,
        _tool_state: Option<&csa_session::state::ToolState>,
        _session: &csa_session::state::MetaSessionState,
        _extra_env: Option<&HashMap<String, String>>,
        _options: TransportOptions<'_>,
    ) -> anyhow::Result<TransportResult> {
        panic!("RecordingTransport::execute should not be called in execute_in tests");
    }

    async fn execute_in(
        &self,
        _prompt: &str,
        _work_dir: &Path,
        _extra_env: Option<&HashMap<String, String>>,
        _stream_mode: csa_process::StreamMode,
        _idle_timeout_seconds: u64,
        initial_response_timeout_seconds: Option<u64>,
    ) -> anyhow::Result<TransportResult> {
        self.captured_timeouts
            .lock()
            .expect("captured_timeouts mutex poisoned")
            .push(initial_response_timeout_seconds);
        Ok(TransportResult {
            execution: csa_process::ExecutionResult {
                summary: "ok".to_string(),
                output: String::new(),
                stderr_output: String::new(),
                exit_code: 0,
                peak_memory_mb: None,
            },
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn codex_cli_executor() -> Executor {
    Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::from_transport(CodexTransport::Cli),
    }
}

#[tokio::test]
async fn test_execute_in_with_transport_preserves_resolved_none_timeout() {
    let executor = codex_cli_executor();
    let captured_timeouts = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        captured_timeouts: Arc::clone(&captured_timeouts),
    };
    let temp_dir = tempfile::tempdir().expect("tempdir");

    executor
        .execute_in_with_transport_via(
            &transport,
            ExecuteInTransportRequest {
                prompt: "prompt",
                work_dir: temp_dir.path(),
                extra_env: None,
                stream_mode: csa_process::StreamMode::BufferOnly,
                idle_timeout_seconds: 30,
                initial_response_timeout_seconds: None,
            },
        )
        .await
        .expect("execute_in_with_transport_via should forward the already-resolved None");

    assert_eq!(
        *captured_timeouts
            .lock()
            .expect("captured_timeouts mutex poisoned"),
        vec![None],
        "execute_in_with_transport must preserve None as disabled instead of re-defaulting"
    );
}

#[tokio::test]
async fn test_execute_in_with_transport_preserves_zero_timeout_sentinel() {
    let executor = codex_cli_executor();
    let captured_timeouts = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        captured_timeouts: Arc::clone(&captured_timeouts),
    };
    let temp_dir = tempfile::tempdir().expect("tempdir");

    executor
        .execute_in_with_transport_via(
            &transport,
            ExecuteInTransportRequest {
                prompt: "prompt",
                work_dir: temp_dir.path(),
                extra_env: None,
                stream_mode: csa_process::StreamMode::BufferOnly,
                idle_timeout_seconds: 30,
                initial_response_timeout_seconds: Some(0),
            },
        )
        .await
        .expect("execute_in_with_transport_via should forward the already-resolved Some(0)");

    assert_eq!(
        *captured_timeouts
            .lock()
            .expect("captured_timeouts mutex poisoned"),
        vec![Some(0)],
        "execute_in_with_transport must preserve Some(0) instead of re-defaulting"
    );
}
