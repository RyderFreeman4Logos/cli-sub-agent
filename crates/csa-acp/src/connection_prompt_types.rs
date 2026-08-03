use super::*;

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    /// Agent output text (tail-only for large sessions; full output stays in the spool file).
    pub output: String,
    pub events: Vec<SessionEvent>,
    pub exit_reason: Option<String>,
    pub timed_out: bool,
    /// Incrementally collected metadata from the event stream.
    pub metadata: StreamingMetadata,
}

#[derive(Debug, Clone)]
pub struct PromptIoOptions<'a> {
    pub stream_stdout_to_stderr: bool,
    pub output_spool: Option<&'a Path>,
    pub spool_max_bytes: u64,
    pub keep_rotated_spool: bool,
    pub tool_output_compaction: Option<ToolOutputCompactionConfig>,
}

impl Default for PromptIoOptions<'_> {
    fn default() -> Self {
        Self {
            stream_stdout_to_stderr: false,
            output_spool: None,
            spool_max_bytes: DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: DEFAULT_SPOOL_KEEP_ROTATED,
            tool_output_compaction: None,
        }
    }
}
