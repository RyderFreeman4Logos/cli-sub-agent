use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Enable automatic memory capture from PostRun hook.
    pub auto_capture: bool,
    /// Enable memory injection into csa run prompts.
    pub inject: bool,
    /// Maximum tokens for injected memory context.
    pub inject_token_budget: u32,
    /// Entry count threshold to trigger consolidation suggestion.
    pub consolidation_threshold: u32,
    /// LLM API configuration for memory operations.
    pub llm: MemoryLlmConfig,
    /// Ephemeral fallback configuration.
    pub ephemeral: MemoryEphemeralConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            auto_capture: false,
            inject: false,
            inject_token_budget: 2000,
            consolidation_threshold: 100,
            llm: MemoryLlmConfig::default(),
            ephemeral: MemoryEphemeralConfig::default(),
        }
    }
}

impl MemoryConfig {
    pub fn is_default(&self) -> bool {
        !self.auto_capture
            && !self.inject
            && self.inject_token_budget == 2000
            && self.consolidation_threshold == 100
            && self.llm.is_default()
            && self.ephemeral.is_default()
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MemoryLlmConfig {
    /// Enable direct LLM API calls for memory operations.
    pub enabled: bool,
    /// OpenAI-compatible API base URL.
    ///
    /// Examples:
    /// - OpenAI:       https://api.openai.com/v1
    /// - Azure:        https://{resource}.openai.azure.com/openai/deployments/{deploy}/v1
    /// - Google AI:    https://generativelanguage.googleapis.com/v1beta/openai
    /// - Groq:         https://api.groq.com/openai/v1
    /// - DeepSeek:     https://api.deepseek.com/v1
    /// - Local Ollama: http://localhost:11434/v1
    /// - Local vLLM:   http://localhost:8000/v1
    /// - LiteLLM:      http://localhost:4000/v1
    pub base_url: String,
    /// API key for authentication.
    pub api_key: String,
    /// Comma-separated model list for failover.
    ///
    /// First model is primary; on 429/quota exhaustion, auto-switch to next.
    /// Example: "gpt-5.3-codex-spark,gpt-5.1-codex-mini"
    pub models: String,
}

impl MemoryLlmConfig {
    pub fn is_default(&self) -> bool {
        !self.enabled
            && self.base_url.is_empty()
            && self.api_key.is_empty()
            && self.models.is_empty()
    }

    pub fn redacted_api_key(&self) -> String {
        mask_api_key(&self.api_key)
    }

    pub fn redacted_for_display(&self) -> Self {
        let mut redacted = self.clone();
        redacted.api_key = redacted.redacted_api_key();
        redacted
    }
}

impl fmt::Debug for MemoryLlmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryLlmConfig")
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("api_key", &self.redacted_api_key())
            .field("models", &self.models)
            .finish()
    }
}

impl fmt::Display for MemoryLlmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "enabled={}, base_url=\"{}\", api_key=\"{}\", models=\"{}\"",
            self.enabled,
            self.base_url,
            self.redacted_api_key(),
            self.models
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MemoryEphemeralConfig {
    /// Enable ephemeral fallback (csa run --ephemeral).
    pub enabled: bool,
    /// Model spec in tool/provider/model/thinking format.
    /// Example: "codex/openai/gpt-5.3-codex/low"
    pub model_spec: String,
}

impl MemoryEphemeralConfig {
    pub fn is_default(&self) -> bool {
        !self.enabled && self.model_spec.is_empty()
    }
}

fn mask_api_key(api_key: &str) -> String {
    if api_key.is_empty() {
        return String::new();
    }

    let char_count = api_key.chars().count();
    let prefix: String = api_key.chars().take(3).collect();
    let suffix: String = api_key.chars().skip(char_count.saturating_sub(4)).collect();

    if char_count <= 4 {
        format!("***{suffix}")
    } else {
        format!("{prefix}...{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryConfig, MemoryLlmConfig};
    use crate::ProjectConfig;

    #[derive(Debug, serde::Deserialize)]
    struct MemoryEnvelope {
        #[serde(default)]
        memory: MemoryConfig,
    }

    #[test]
    fn test_memory_config_defaults() {
        let parsed: MemoryEnvelope = toml::from_str("[memory]\n").unwrap();
        assert!(!parsed.memory.auto_capture);
        assert!(!parsed.memory.inject);
        assert_eq!(parsed.memory.inject_token_budget, 2000);
        assert_eq!(parsed.memory.consolidation_threshold, 100);
        assert!(!parsed.memory.llm.enabled);
        assert!(!parsed.memory.ephemeral.enabled);
    }

    #[test]
    fn test_memory_config_full() {
        let toml = r#"
[memory]
auto_capture = true
inject = true
inject_token_budget = 4096
consolidation_threshold = 250

[memory.llm]
enabled = true
base_url = "https://api.openai.com/v1"
api_key = "sk-example-1234"
models = "gpt-5.3-codex-spark,gpt-5.1-codex-mini"

[memory.ephemeral]
enabled = true
model_spec = "codex/openai/gpt-5.3-codex/low"
"#;
        let parsed: MemoryEnvelope = toml::from_str(toml).unwrap();
        assert!(parsed.memory.auto_capture);
        assert!(parsed.memory.inject);
        assert_eq!(parsed.memory.inject_token_budget, 4096);
        assert_eq!(parsed.memory.consolidation_threshold, 250);
        assert!(parsed.memory.llm.enabled);
        assert_eq!(parsed.memory.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(parsed.memory.llm.api_key, "sk-example-1234");
        assert_eq!(
            parsed.memory.llm.models,
            "gpt-5.3-codex-spark,gpt-5.1-codex-mini"
        );
        assert!(parsed.memory.ephemeral.enabled);
        assert_eq!(
            parsed.memory.ephemeral.model_spec,
            "codex/openai/gpt-5.3-codex/low"
        );
    }

    #[test]
    fn test_memory_config_models_parsing() {
        let toml = r#"
[memory]
[memory.llm]
models = "model-a,model-b,model-c"
"#;
        let parsed: MemoryEnvelope = toml::from_str(toml).unwrap();
        assert_eq!(parsed.memory.llm.models, "model-a,model-b,model-c");
    }

    #[test]
    fn test_memory_config_backward_compat() {
        let parsed: ProjectConfig = toml::from_str(
            r#"
schema_version = 1
[project]
name = "compat-test"
"#,
        )
        .unwrap();
        assert_eq!(parsed.memory.inject_token_budget, 2000);
        assert_eq!(parsed.memory.consolidation_threshold, 100);
        assert!(!parsed.memory.auto_capture);
        assert!(!parsed.memory.inject);
        assert!(!parsed.memory.llm.enabled);
        assert!(!parsed.memory.ephemeral.enabled);
    }

    #[test]
    fn test_memory_llm_debug_masks_api_key() {
        let llm = MemoryLlmConfig {
            enabled: true,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "sk-super-secret-5982".to_string(),
            models: "gpt-5.3-codex-spark".to_string(),
        };
        let debug = format!("{llm:?}");
        assert!(!debug.contains("sk-super-secret-5982"));
        assert!(debug.contains("sk-...5982"));
    }
}
