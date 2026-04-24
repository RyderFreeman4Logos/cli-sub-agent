//! OpenAI-compatible HTTP API transport.
//!
//! Pure HTTP transport for OpenAI-compatible API endpoints (e.g., litellm, vllm,
//! local proxy servers). No CLI process, no cgroup sandbox, no signal handling.
//! Sends prompts via the `/v1/chat/completions` endpoint and collects the response.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use csa_process::ExecutionResult;
use csa_session::state::{MetaSessionState, ToolState};
use serde::{Deserialize, Serialize};

use crate::transport::{
    ResolvedTimeout, Transport, TransportOptions, TransportResult, build_ephemeral_meta_session,
};

/// Environment variable names for OpenAI-compat configuration.
const ENV_BASE_URL: &str = "OPENAI_COMPAT_BASE_URL";
const ENV_API_KEY: &str = "OPENAI_COMPAT_API_KEY";
const ENV_MODEL: &str = "OPENAI_COMPAT_MODEL";

/// Configuration for the OpenAI-compatible API endpoint.
#[derive(Debug, Clone)]
pub struct OpenaiCompatConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// HTTP-only transport for OpenAI-compatible APIs.
///
/// Config resolution order (per field):
/// 1. `extra_env` passed at execution time (from `[tools.openai-compat.env]` in global config)
/// 2. System environment variables
/// 3. `default_model` set at construction time (from executor's model_override)
#[derive(Debug, Clone)]
pub struct OpenaiCompatTransport {
    /// Model from the executor's model_override (fallback when env is unset).
    default_model: Option<String>,
}

impl OpenaiCompatTransport {
    pub fn new(default_model: Option<String>) -> Self {
        Self { default_model }
    }

    /// Create with explicit config (for tests or direct construction).
    pub fn with_config(config: OpenaiCompatConfig) -> Self {
        // Store model as default; base_url and api_key will be re-resolved at execute time.
        Self {
            default_model: Some(config.model),
        }
    }

    /// Resolve a config value from extra_env, then system env.
    fn resolve_env(key: &str, extra_env: Option<&HashMap<String, String>>) -> Option<String> {
        extra_env
            .and_then(|env| env.get(key).cloned())
            .or_else(|| std::env::var(key).ok())
    }

    /// Resolve full config from extra_env + system env + defaults.
    fn resolve_config(
        &self,
        extra_env: Option<&HashMap<String, String>>,
    ) -> Result<OpenaiCompatConfig> {
        let base_url = Self::resolve_env(ENV_BASE_URL, extra_env).ok_or_else(|| {
            anyhow::anyhow!(
                "OpenAI-compat base URL not configured. Set {ENV_BASE_URL} in [tools.openai-compat.env] \
                 or as an environment variable."
            )
        })?;

        let api_key = Self::resolve_env(ENV_API_KEY, extra_env).ok_or_else(|| {
            anyhow::anyhow!(
                "OpenAI-compat API key not configured. Set {ENV_API_KEY} in [tools.openai-compat.env] \
                 or as an environment variable."
            )
        })?;

        let model = Self::resolve_env(ENV_MODEL, extra_env)
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenAI-compat model not configured. Set {ENV_MODEL} in [tools.openai-compat.env], \
                 use --model, or set default_model in [tools.openai-compat]."
                )
            })?;

        Ok(OpenaiCompatConfig {
            base_url,
            api_key,
            model,
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatUsage {
    #[serde(default)]
    total_tokens: u64,
}

#[async_trait]
impl Transport for OpenaiCompatTransport {
    fn mode(&self) -> crate::transport::TransportMode {
        crate::transport::TransportMode::OpenaiCompat
    }

    fn capabilities(&self) -> crate::transport::TransportCapabilities {
        crate::transport::TransportCapabilities {
            streaming: false,
            session_resume: false,
            session_fork: false,
            typed_events: false,
        }
    }

    async fn execute(
        &self,
        prompt: &str,
        _tool_state: Option<&ToolState>,
        _session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        _options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let config = self.resolve_config(extra_env)?;
        let url = format!(
            "{}/v1/chat/completions",
            config.base_url.trim_end_matches('/')
        );

        let request_body = ChatRequest {
            model: &config.model,
            messages: vec![ChatMessage {
                role: "user",
                content: prompt,
            }],
            max_tokens: Some(16384),
        };

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to OpenAI-compatible API")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            bail!(
                "OpenAI-compat API returned HTTP {}: {}",
                status.as_u16(),
                error_body
            );
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .context("Failed to parse OpenAI-compat API response")?;

        let output = chat_response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let token_info = chat_response
            .usage
            .map(|u| format!("total_tokens: {}", u.total_tokens))
            .unwrap_or_default();

        let summary = output.lines().next_back().unwrap_or("").to_string();

        Ok(TransportResult {
            execution: ExecutionResult {
                output,
                stderr_output: token_info,
                summary,
                exit_code: 0,
                peak_memory_mb: None,
            },
            provider_session_id: None,
            events: Vec::new(),
            metadata: Default::default(),
        })
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        _stream_mode: csa_process::StreamMode,
        _idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        let session = build_ephemeral_meta_session(work_dir);
        self.execute(
            prompt,
            None,
            &session,
            extra_env,
            TransportOptions {
                stream_mode: csa_process::StreamMode::BufferOnly,
                idle_timeout_seconds: csa_process::DEFAULT_IDLE_TIMEOUT_SECS,
                acp_crash_max_attempts: 2,
                initial_response_timeout,
                liveness_dead_seconds: csa_process::DEFAULT_LIVENESS_DEAD_SECS,
                stdin_write_timeout_seconds: csa_process::DEFAULT_STDIN_WRITE_TIMEOUT_SECS,
                acp_init_timeout_seconds: 120,
                termination_grace_period_seconds:
                    csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS,
                output_spool: None,
                output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
                output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
                setting_sources: None,
                sandbox: None,
                thinking_budget: None,
            },
        )
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_compat_transport_default_model() {
        let transport = OpenaiCompatTransport::new(Some("gemini-flash".to_string()));
        assert_eq!(transport.default_model.as_deref(), Some("gemini-flash"));
    }

    #[test]
    fn test_openai_compat_transport_with_config() {
        let config = OpenaiCompatConfig {
            base_url: "http://localhost:8317".to_string(),
            api_key: "sk-test".to_string(),
            model: "gemini-flash".to_string(),
        };
        let transport = OpenaiCompatTransport::with_config(config);
        assert_eq!(transport.default_model.as_deref(), Some("gemini-flash"));
    }

    #[test]
    fn test_resolve_config_from_extra_env() {
        let transport = OpenaiCompatTransport::new(None);
        let mut env = HashMap::new();
        env.insert(
            ENV_BASE_URL.to_string(),
            "http://localhost:8317".to_string(),
        );
        env.insert(ENV_API_KEY.to_string(), "sk-test".to_string());
        env.insert(ENV_MODEL.to_string(), "gemini-flash".to_string());
        let config = transport.resolve_config(Some(&env)).unwrap();
        assert_eq!(config.base_url, "http://localhost:8317");
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.model, "gemini-flash");
    }

    #[test]
    fn test_resolve_config_model_fallback_to_default() {
        let transport = OpenaiCompatTransport::new(Some("gemini-pro".to_string()));
        let mut env = HashMap::new();
        env.insert(
            ENV_BASE_URL.to_string(),
            "http://localhost:8317".to_string(),
        );
        env.insert(ENV_API_KEY.to_string(), "sk-test".to_string());
        let config = transport.resolve_config(Some(&env)).unwrap();
        assert_eq!(config.model, "gemini-pro");
    }

    #[test]
    fn test_resolve_config_missing_base_url_errors() {
        let transport = OpenaiCompatTransport::new(Some("gemini-flash".to_string()));
        let env = HashMap::new();
        let err = transport.resolve_config(Some(&env)).unwrap_err();
        assert!(err.to_string().contains(ENV_BASE_URL));
    }

    #[test]
    fn test_chat_request_serialization() {
        let request = ChatRequest {
            model: "gemini-flash",
            messages: vec![ChatMessage {
                role: "user",
                content: "Hello",
            }],
            max_tokens: Some(4096),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gemini-flash"));
        assert!(json.contains("Hello"));
        assert!(json.contains("4096"));
    }

    #[test]
    fn test_chat_response_deserialization() {
        let json = r#"{
            "choices": [{"message": {"content": "Hello back!", "role": "assistant"}}],
            "usage": {"total_tokens": 42, "prompt_tokens": 10, "completion_tokens": 32}
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("Hello back!")
        );
        assert_eq!(resp.usage.unwrap().total_tokens, 42);
    }

    #[test]
    fn test_chat_response_without_usage() {
        let json = r#"{"choices": [{"message": {"content": "Hi"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert!(resp.usage.is_none());
    }
}
