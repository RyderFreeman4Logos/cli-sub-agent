//! Token usage and cost parsing from tool output.

use csa_session::TokenUsage;

/// Parse token usage from tool output (best-effort, returns None on failure).
///
/// Looks for common patterns in stdout/stderr:
/// - "tokens: N" or "Tokens: N" or "total_tokens: N"
/// - "input_tokens: N" / "output_tokens: N"
/// - "cost: $N.NN" or "estimated_cost: $N.NN"
pub(crate) fn parse_token_usage(output: &str) -> Option<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found_any = false;

    // Simple pattern matching without regex
    for line in output.lines() {
        let line_lower = line.to_lowercase();

        // Parse input_tokens
        if let Some(pos) = line_lower.find("input_tokens")
            && let Some(val) = extract_number(&line[pos..])
        {
            usage.input_tokens = Some(val);
            found_any = true;
        }

        // Parse output_tokens
        if let Some(pos) = line_lower.find("output_tokens")
            && let Some(val) = extract_number(&line[pos..])
        {
            usage.output_tokens = Some(val);
            found_any = true;
        }

        // Parse total_tokens
        if let Some(pos) = line_lower.find("total_tokens") {
            if let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        } else if let Some(pos) = line_lower.find("tokens:") {
            // Only match standalone "tokens:" — skip if preceded by a letter or
            // underscore (e.g. "input_tokens:" or "output_tokens:" already
            // handled above).
            let prev = line_lower.as_bytes().get(pos.wrapping_sub(1)).copied();
            let is_standalone = pos == 0 || !matches!(prev, Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'));
            if is_standalone && let Some(val) = extract_number(&line[pos..]) {
                usage.total_tokens = Some(val);
                found_any = true;
            }
        }

        // Parse cost (look for "$N.NN" pattern)
        if line_lower.contains("cost")
            && let Some(val) = extract_cost(line)
        {
            usage.estimated_cost_usd = Some(val);
            found_any = true;
        }
    }

    // Calculate total_tokens if not found but input/output are available
    if usage.total_tokens.is_none()
        && let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens)
    {
        usage.total_tokens = Some(input + output);
        found_any = true;
    }

    if found_any { Some(usage) } else { None }
}

/// Extract a number after colon or equals sign.
pub(crate) fn extract_number(text: &str) -> Option<u64> {
    // Find colon or equals
    let start = text.find(':')?;
    let after_colon = &text[start + 1..];

    // Take first word after colon
    let num_str: String = after_colon
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    num_str.parse().ok()
}

/// Extract cost value after $ sign.
pub(crate) fn extract_cost(text: &str) -> Option<f64> {
    let start = text.find('$')?;
    let after_dollar = &text[start + 1..];

    // Take digits and decimal point
    let num_str: String = after_dollar
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    num_str.parse().ok()
}
