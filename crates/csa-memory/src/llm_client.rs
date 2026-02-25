use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use crate::MemoryEntry;

const DEFAULT_COOLDOWN: Duration = Duration::from_secs(600);

/// Extracted fact from session output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub content: String,
    pub tags: Vec<String>,
}

/// Trait for LLM operations needed by the memory system
#[async_trait]
pub trait MemoryLlmClient: Send + Sync {
    /// Extract structured facts from raw text
    async fn extract_facts(&self, text: &str) -> Result<Vec<Fact>>;
    /// Summarize multiple memory entries into a consolidated one
    async fn summarize(&self, entries: &[MemoryEntry]) -> Result<String>;
}

#[derive(Debug)]
pub struct ApiClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
    rotator: Mutex<ModelRotator>,
}

impl ApiClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models_csv: &str,
    ) -> Result<Self> {
        let models: Vec<String> = models_csv
            .split(',')
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(ToOwned::to_owned)
            .collect();

        if models.is_empty() {
            bail!("at least one model is required for ApiClient");
        }

        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
            rotator: Mutex::new(ModelRotator::new(models)),
        })
    }

    async fn run_chat_completion(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        loop {
            let model = {
                let mut rotator = self
                    .rotator
                    .lock()
                    .map_err(|_| anyhow!("model rotator poisoned"))?;
                if rotator.all_exhausted() {
                    bail!("all memory llm models are currently in cooldown");
                }
                rotator.next_available().to_string()
            };

            let url = format!("{}/chat/completions", self.base_url);
            let response = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&json!({
                    "model": model,
                    "messages": [
                        {"role": "system", "content": system_prompt},
                        {"role": "user", "content": user_prompt}
                    ],
                    "temperature": 0.1
                }))
                .send()
                .await
                .with_context(|| format!("memory llm request failed for model {model}"))?;

            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .text()
                .await
                .with_context(|| format!("failed to read response body for model {model}"))?;

            if status.is_success() {
                return parse_completion_content(&body);
            }

            if is_rate_or_quota_error(status, &body) {
                let cooldown = parse_retry_after(&headers).unwrap_or(DEFAULT_COOLDOWN);
                let (has_next, next_model) = {
                    let mut rotator = self
                        .rotator
                        .lock()
                        .map_err(|_| anyhow!("model rotator poisoned"))?;
                    rotator.mark_exhausted(&model, cooldown);
                    let has_next = !rotator.all_exhausted();
                    let next_model = if has_next {
                        Some(rotator.peek_next_available().to_string())
                    } else {
                        None
                    };
                    (has_next, next_model)
                };

                if has_next {
                    if let Some(new_model) = next_model {
                        warn!(
                            "memory LLM failover: {} -> {} (cooldown {}s)",
                            model,
                            new_model,
                            cooldown.as_secs()
                        );
                    }
                    continue;
                }

                bail!(
                    "all memory llm models exhausted after rate/quota limit; last model: {model}, status: {status}"
                );
            }

            return Err(anyhow!(
                "memory llm request failed for model {model}: status {status}, body {body}"
            ));
        }
    }
}

#[async_trait]
impl MemoryLlmClient for ApiClient {
    async fn extract_facts(&self, text: &str) -> Result<Vec<Fact>> {
        let system_prompt = "Extract factual statements from the input and return strict JSON array [{\"content\":\"...\",\"tags\":[\"...\"]}].";
        let content = self.run_chat_completion(system_prompt, text).await?;

        if let Ok(facts) = serde_json::from_str::<Vec<Fact>>(&content) {
            return Ok(facts);
        }

        #[derive(Deserialize)]
        struct WrappedFacts {
            facts: Vec<Fact>,
        }

        if let Ok(wrapper) = serde_json::from_str::<WrappedFacts>(&content) {
            return Ok(wrapper.facts);
        }

        Ok(vec![Fact {
            content: content.trim().to_string(),
            tags: Vec::new(),
        }])
    }

    async fn summarize(&self, entries: &[MemoryEntry]) -> Result<String> {
        if entries.is_empty() {
            return Ok(String::new());
        }

        let merged = entries
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let system_prompt =
            "Summarize the provided memory entries into one concise consolidated memory.";
        self.run_chat_completion(system_prompt, &merged).await
    }
}

#[derive(Debug, Clone)]
pub struct ModelRotator {
    models: Vec<String>,
    cooldowns: HashMap<String, Instant>,
    current_index: usize,
}

impl ModelRotator {
    pub fn new(models: Vec<String>) -> Self {
        assert!(
            !models.is_empty(),
            "ModelRotator requires at least one model"
        );
        Self {
            models,
            cooldowns: HashMap::new(),
            current_index: 0,
        }
    }

    /// Get next available model (skip models still in cooldown)
    pub fn next_available(&mut self) -> &str {
        self.purge_expired();
        let total = self.models.len();

        for _ in 0..total {
            let index = self.current_index % total;
            self.current_index = (self.current_index + 1) % total;
            let model = &self.models[index];
            if !self.in_cooldown(model) {
                return model;
            }
        }

        &self.models[self.current_index % total]
    }

    /// Peek next available model without advancing rotation index.
    pub fn peek_next_available(&mut self) -> &str {
        self.purge_expired();
        let total = self.models.len();
        let start_index = self.current_index % total;

        for offset in 0..total {
            let index = (start_index + offset) % total;
            let model = &self.models[index];
            if !self.in_cooldown(model) {
                return model;
            }
        }

        &self.models[start_index]
    }

    /// Mark a model as exhausted with cooldown duration
    pub fn mark_exhausted(&mut self, model: &str, cooldown: Duration) {
        self.cooldowns
            .insert(model.to_string(), Instant::now() + cooldown);
    }

    /// Check if all models are in cooldown
    pub fn all_exhausted(&self) -> bool {
        let now = Instant::now();
        self.models.iter().all(|model| {
            self.cooldowns
                .get(model)
                .is_some_and(|cooldown_until| *cooldown_until > now)
        })
    }

    fn in_cooldown(&self, model: &str) -> bool {
        let now = Instant::now();
        self.cooldowns
            .get(model)
            .is_some_and(|cooldown_until| *cooldown_until > now)
    }

    fn purge_expired(&mut self) {
        let now = Instant::now();
        self.cooldowns.retain(|_, until| *until > now);
    }
}

fn is_rate_or_quota_error(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return true;
    }

    let body_lower = body.to_ascii_lowercase();
    body_lower.contains("rate_limit")
        || body_lower.contains("quota")
        || body_lower.contains("insufficient_quota")
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();

    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = DateTime::parse_from_rfc2822(raw).ok()?.with_timezone(&Utc);
    let now = Utc::now();
    let seconds = (retry_at - now).num_seconds().max(0) as u64;
    Some(Duration::from_secs(seconds))
}

fn parse_completion_content(body: &str) -> Result<String> {
    let value: Value =
        serde_json::from_str(body).context("failed to parse completion response JSON")?;
    let content = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing choices[0].message.content in completion response"))?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemorySource, NoopClient};
    use chrono::Utc;
    use reqwest::header::HeaderValue;
    use ulid::Ulid;

    #[test]
    fn test_model_rotator_basic() {
        let mut rotator = ModelRotator::new(vec!["gpt-a".to_string(), "gpt-b".to_string()]);
        assert_eq!(rotator.next_available(), "gpt-a");
    }

    #[test]
    fn test_model_rotator_failover() {
        let mut rotator = ModelRotator::new(vec!["gpt-a".to_string(), "gpt-b".to_string()]);
        let first = rotator.next_available().to_string();
        rotator.mark_exhausted(&first, Duration::from_secs(60));
        assert_eq!(rotator.next_available(), "gpt-b");
    }

    #[test]
    fn test_model_rotator_cooldown_expiry() {
        let mut rotator = ModelRotator::new(vec!["gpt-a".to_string(), "gpt-b".to_string()]);
        rotator.mark_exhausted("gpt-a", Duration::from_secs(0));
        assert_eq!(rotator.next_available(), "gpt-a");
    }

    #[test]
    fn test_model_rotator_all_exhausted() {
        let mut rotator = ModelRotator::new(vec!["gpt-a".to_string(), "gpt-b".to_string()]);
        rotator.mark_exhausted("gpt-a", Duration::from_secs(60));
        rotator.mark_exhausted("gpt-b", Duration::from_secs(60));
        assert!(rotator.all_exhausted());
    }

    #[test]
    fn test_model_rotator_peek_does_not_advance() {
        let mut rotator = ModelRotator::new(vec![
            "gpt-a".to_string(),
            "gpt-b".to_string(),
            "gpt-c".to_string(),
        ]);
        assert_eq!(rotator.next_available(), "gpt-a");
        rotator.mark_exhausted("gpt-b", Duration::from_secs(60));

        assert_eq!(rotator.peek_next_available(), "gpt-c");
        assert_eq!(rotator.next_available(), "gpt-c");
    }

    #[test]
    fn test_retry_after_parsing() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));

        let retry_after = parse_retry_after(&headers);
        assert_eq!(retry_after, Some(Duration::from_secs(120)));
    }

    #[tokio::test]
    async fn test_noop_client() {
        let client = NoopClient::default();
        let input = "session output text";
        let facts = client
            .extract_facts(input)
            .await
            .expect("extract_facts should succeed");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, input);

        let entry = MemoryEntry {
            id: Ulid::new(),
            timestamp: Utc::now(),
            project: Some("proj".to_string()),
            tool: Some("codex".to_string()),
            session_id: Some("session-1".to_string()),
            tags: vec!["test".to_string()],
            content: "first line".to_string(),
            facts: vec![],
            source: MemorySource::Manual,
            valid_from: None,
            valid_until: None,
        };
        let summary = client
            .summarize(&[entry])
            .await
            .expect("summarize should succeed");
        assert_eq!(summary, "first line");
    }
}
