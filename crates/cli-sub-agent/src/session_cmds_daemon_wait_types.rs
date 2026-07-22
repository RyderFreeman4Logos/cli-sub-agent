use std::path::Path;

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionWaitOutputMode {
    CompactText,
    CompactJson,
    Verbose,
}

impl SessionWaitOutputMode {
    pub(crate) fn from_flags(verbose: bool, json: bool) -> Self {
        if verbose || std::env::var("CSA_WAIT_VERBOSE").is_ok_and(|value| value == "1") {
            return Self::Verbose;
        }
        if json {
            return Self::CompactJson;
        }
        Self::CompactText
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitLoopTiming {
    pub(crate) poll_interval: std::time::Duration,
    pub(crate) memory_sample_interval: std::time::Duration,
}

impl Default for WaitLoopTiming {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(1),
            memory_sample_interval: std::time::Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitBehavior {
    pub(crate) wait_timeout_secs: u64,
    pub(crate) memory_warn_mb: Option<u64>,
    pub(crate) timing: WaitLoopTiming,
}

impl WaitBehavior {
    pub(super) fn new(wait_timeout_secs: u64, memory_warn_mb: Option<u64>) -> Self {
        Self {
            wait_timeout_secs,
            memory_warn_mb,
            timing: WaitLoopTiming::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct WaitExecutionOptions {
    pub(super) behavior: WaitBehavior,
    pub(super) output_mode: SessionWaitOutputMode,
    pub(super) caller_identity: super::WaitCallerIdentity,
    pub(super) model_provider: Option<csa_config::ModelProvider>,
    #[cfg(test)]
    pub(super) current_time_for_test: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg(test)]
    pub(super) session_live_for_test: Option<bool>,
}

impl WaitExecutionOptions {
    pub(super) fn current_time(&self) -> chrono::DateTime<chrono::Utc> {
        #[cfg(test)]
        if let Some(current_time) = self.current_time_for_test {
            return current_time;
        }

        chrono::Utc::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitReconciliationOutcome {
    pub(crate) result_became_available: bool,
    pub(crate) synthetic: bool,
}

type ReconcileEmitter<'a> =
    Box<dyn FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome> + 'a>;
type CompletionSignalEmitter<'a> = Box<dyn FnMut(&str, &str, i32, bool, bool) + 'a>;
type MemorySampler<'a> = Box<dyn FnMut(&Path, &str) -> std::io::Result<u64> + 'a>;
type MemoryWarnEmitter<'a> = Box<dyn FnMut(&str, u64, u64) + 'a>;
type TerminalOutputEmitter<'a> = Box<
    dyn FnMut(
            &Path,
            &str,
            Option<&csa_session::SessionResult>,
            SessionWaitOutputMode,
        ) -> Result<bool>
        + 'a,
>;
type NextStepEmitter<'a> = Box<dyn FnMut(&Path) -> Result<()> + 'a>;

/// Testable side effects and probes used by the session-wait polling loop.
pub(crate) struct WaitEmitters<'a> {
    pub(crate) reconcile_dead_active_session: ReconcileEmitter<'a>,
    pub(crate) emit_completion_signal: CompletionSignalEmitter<'a>,
    pub(crate) sample_session_tree_rss_mb: MemorySampler<'a>,
    pub(crate) emit_memory_warn_marker: MemoryWarnEmitter<'a>,
    pub(crate) emit_terminal_output: TerminalOutputEmitter<'a>,
    pub(crate) emit_next_step: NextStepEmitter<'a>,
}
