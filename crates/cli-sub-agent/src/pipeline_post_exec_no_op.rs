//! No-op exit classification helpers.

use csa_session::TokenUsage;

/// Sessions shorter than this threshold (in seconds) that exit 0 with zero
/// tool calls in sa-mode are classified as no-op exits.
pub(super) const ELAPSED_THRESHOLD_SECS: i64 = 60;
const MEANINGFUL_OUTPUT_TOKENS: u64 = 1000;

pub(super) fn has_meaningful_reasoning_output(
    token_usage: &Option<TokenUsage>,
    transport_output_tokens: Option<u64>,
) -> bool {
    [
        token_usage.as_ref().and_then(|usage| usage.output_tokens),
        transport_output_tokens,
    ]
    .into_iter()
    .flatten()
    .max()
    .is_some_and(|output_tokens| output_tokens > MEANINGFUL_OUTPUT_TOKENS)
}
