//! Adapter from CSA `Executor` to `agent-teams` backend traits.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_teams::{
    AgentBackend, AgentOutput, AgentSession, BackendType, Error as AgentTeamsError, SpawnConfig,
};
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::{Executor, ThinkingBudget};

const OUTPUT_CHANNEL_CAPACITY: usize = 128;

/// Thin wrapper that exposes CSA's existing `Executor` as an `agent-teams` backend.
///
/// This adapter is additive: it delegates execution to `Executor::execute_in` and
/// does not change CSA's existing execution behavior.
#[derive(Debug, Clone)]
pub struct ExecutorAgentBackend {
    executor: Executor,
    base_env: HashMap<String, String>,
}

impl ExecutorAgentBackend {
    /// Create a backend adapter with no extra environment variables.
    pub fn new(executor: Executor) -> Self {
        Self {
            executor,
            base_env: HashMap::new(),
        }
    }

    /// Create a backend adapter with base environment variables.
    pub fn with_env(executor: Executor, base_env: HashMap<String, String>) -> Self {
        Self { executor, base_env }
    }

    /// Access the wrapped CSA executor.
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    fn backend_type_for_executor(executor: &Executor) -> BackendType {
        match executor {
            Executor::ClaudeCode { .. } => BackendType::ClaudeCode,
            Executor::GeminiCli { .. } => BackendType::GeminiCli,
            Executor::Codex { .. } | Executor::Opencode { .. } => BackendType::Codex,
        }
    }

    fn apply_spawn_overrides(
        mut executor: Executor,
        agent_name: &str,
        model: Option<String>,
        reasoning_effort: Option<String>,
    ) -> Result<Executor, AgentTeamsError> {
        if let Some(model_override) = model {
            match &mut executor {
                Executor::GeminiCli {
                    model_override: m, ..
                } => *m = Some(model_override.clone()),
                Executor::Opencode {
                    model_override: m, ..
                } => *m = Some(model_override.clone()),
                Executor::Codex {
                    model_override: m, ..
                } => *m = Some(model_override.clone()),
                Executor::ClaudeCode {
                    model_override: m, ..
                } => *m = Some(model_override),
            }
        }

        if let Some(effort) = reasoning_effort {
            let budget =
                ThinkingBudget::parse(&effort).map_err(|e| AgentTeamsError::SpawnFailed {
                    name: agent_name.to_string(),
                    reason: e.to_string(),
                })?;

            match &mut executor {
                Executor::GeminiCli {
                    thinking_budget, ..
                } => *thinking_budget = Some(budget.clone()),
                Executor::Opencode {
                    thinking_budget, ..
                } => *thinking_budget = Some(budget.clone()),
                Executor::Codex {
                    thinking_budget, ..
                } => *thinking_budget = Some(budget.clone()),
                Executor::ClaudeCode {
                    thinking_budget, ..
                } => *thinking_budget = Some(budget),
            }
        }

        Ok(executor)
    }
}

#[async_trait]
impl AgentBackend for ExecutorAgentBackend {
    fn backend_type(&self) -> BackendType {
        Self::backend_type_for_executor(&self.executor)
    }

    async fn spawn(&self, config: SpawnConfig) -> agent_teams::Result<Box<dyn AgentSession>> {
        let SpawnConfig {
            name,
            prompt,
            model,
            cwd,
            reasoning_effort,
            env,
            ..
        } = config;

        let cwd = match cwd {
            Some(path) => path,
            None => std::env::current_dir().map_err(|e| AgentTeamsError::SpawnFailed {
                name: name.clone(),
                reason: e.to_string(),
            })?,
        };

        let executor =
            Self::apply_spawn_overrides(self.executor.clone(), &name, model, reasoning_effort)?;

        let mut merged_env = self.base_env.clone();
        merged_env.extend(env);

        Ok(Box::new(ExecutorAgentSession::new(
            name, prompt, executor, cwd, merged_env,
        )))
    }
}

#[derive(Debug)]
pub struct ExecutorAgentSession {
    name: String,
    system_prompt: String,
    first_turn: bool,
    executor: Executor,
    cwd: PathBuf,
    env: HashMap<String, String>,
    alive: Arc<AtomicBool>,
    output_tx: mpsc::Sender<AgentOutput>,
    output_rx: Option<mpsc::Receiver<AgentOutput>>,
}

impl ExecutorAgentSession {
    fn new(
        name: String,
        system_prompt: String,
        executor: Executor,
        cwd: PathBuf,
        env: HashMap<String, String>,
    ) -> Self {
        let (output_tx, output_rx) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
        Self {
            name,
            system_prompt,
            first_turn: true,
            executor,
            cwd,
            env,
            alive: Arc::new(AtomicBool::new(true)),
            output_tx,
            output_rx: Some(output_rx),
        }
    }

    fn compose_prompt(&mut self, input: &str) -> String {
        if self.first_turn {
            self.first_turn = false;
            if self.system_prompt.trim().is_empty() {
                input.to_string()
            } else {
                format!("{}\n\n{}", self.system_prompt, input)
            }
        } else {
            input.to_string()
        }
    }

    async fn emit(&self, output: AgentOutput) -> agent_teams::Result<()> {
        self.output_tx.send(output).await.map_err(|_| {
            self.alive.store(false, Ordering::Relaxed);
            AgentTeamsError::AgentNotAlive {
                name: self.name.clone(),
            }
        })
    }
}

#[async_trait]
impl AgentSession for ExecutorAgentSession {
    fn name(&self) -> &str {
        &self.name
    }

    async fn send_input(&mut self, input: &str) -> agent_teams::Result<()> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(AgentTeamsError::AgentNotAlive {
                name: self.name.clone(),
            });
        }

        let prompt = self.compose_prompt(input);
        let extra_env = if self.env.is_empty() {
            None
        } else {
            Some(&self.env)
        };

        match self
            .executor
            .execute_in(
                &prompt,
                &self.cwd,
                extra_env,
                csa_process::StreamMode::BufferOnly,
            )
            .await
        {
            Ok(result) => {
                if !result.output.is_empty() {
                    self.emit(AgentOutput::Message(result.output)).await?;
                }

                if result.exit_code != 0 {
                    self.alive.store(false, Ordering::Relaxed);
                    let reason = if result.summary.is_empty() {
                        format!("command exited with status {}", result.exit_code)
                    } else {
                        result.summary
                    };
                    let _ = self.emit(AgentOutput::Error(reason.clone())).await;
                    return Err(AgentTeamsError::Other(reason));
                }

                self.emit(AgentOutput::TurnComplete).await
            }
            Err(e) => {
                self.alive.store(false, Ordering::Relaxed);
                let reason = e.to_string();
                let _ = self.emit(AgentOutput::Error(reason.clone())).await;
                Err(AgentTeamsError::Other(reason))
            }
        }
    }

    fn output_receiver(&mut self) -> Option<mpsc::Receiver<AgentOutput>> {
        self.output_rx.take()
    }

    async fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn shutdown(&mut self) -> agent_teams::Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        let _ = self.emit(AgentOutput::Idle).await;
        Ok(())
    }

    async fn force_kill(&mut self) -> agent_teams::Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        let _ = self
            .emit(AgentOutput::Error("force_kill requested".to_string()))
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_executor() -> Executor {
        Executor::Codex {
            model_override: None,
            thinking_budget: None,
        }
    }

    #[test]
    fn backend_type_mapping_matches_supported_variants() {
        assert_eq!(
            ExecutorAgentBackend::backend_type_for_executor(&Executor::ClaudeCode {
                model_override: None,
                thinking_budget: None,
            }),
            BackendType::ClaudeCode
        );
        assert_eq!(
            ExecutorAgentBackend::backend_type_for_executor(&Executor::GeminiCli {
                model_override: None,
                thinking_budget: None,
            }),
            BackendType::GeminiCli
        );
        assert_eq!(
            ExecutorAgentBackend::backend_type_for_executor(&Executor::Codex {
                model_override: None,
                thinking_budget: None,
            }),
            BackendType::Codex
        );
        assert_eq!(
            ExecutorAgentBackend::backend_type_for_executor(&Executor::Opencode {
                model_override: None,
                agent: None,
                thinking_budget: None,
            }),
            BackendType::Codex
        );
    }

    #[test]
    fn apply_spawn_overrides_updates_model_and_budget() {
        let executor = ExecutorAgentBackend::apply_spawn_overrides(
            codex_executor(),
            "reviewer",
            Some("gpt-5".to_string()),
            Some("high".to_string()),
        )
        .expect("spawn overrides should parse");

        match executor {
            Executor::Codex {
                model_override,
                thinking_budget,
            } => {
                assert_eq!(model_override.as_deref(), Some("gpt-5"));
                assert!(matches!(thinking_budget, Some(ThinkingBudget::High)));
            }
            _ => panic!("expected codex executor"),
        }
    }

    #[test]
    fn compose_prompt_prefixes_system_prompt_once() {
        let mut session = ExecutorAgentSession::new(
            "agent".to_string(),
            "system prompt".to_string(),
            codex_executor(),
            PathBuf::from("."),
            HashMap::new(),
        );

        assert_eq!(
            session.compose_prompt("first"),
            "system prompt\n\nfirst".to_string()
        );
        assert_eq!(session.compose_prompt("second"), "second".to_string());
    }

    #[tokio::test]
    async fn output_receiver_can_only_be_taken_once() {
        let mut session = ExecutorAgentSession::new(
            "agent".to_string(),
            String::new(),
            codex_executor(),
            PathBuf::from("."),
            HashMap::new(),
        );

        assert!(session.output_receiver().is_some());
        assert!(session.output_receiver().is_none());
    }
}
