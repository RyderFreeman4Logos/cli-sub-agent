use serde::{Deserialize, Serialize};

/// Caller-visible warning emitted when a writer session leaves a large changed surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LargeDiffWarningReport {
    pub changed_files: usize,
    pub changed_lines: u64,
    pub approx_diff_tokens: usize,
}
