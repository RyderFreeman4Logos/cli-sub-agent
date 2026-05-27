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

#[derive(Debug, Clone, Copy)]
pub(super) struct WaitExecutionOptions {
    pub(super) behavior: WaitBehavior,
    pub(super) output_mode: SessionWaitOutputMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitReconciliationOutcome {
    pub(crate) result_became_available: bool,
    pub(crate) synthetic: bool,
}
