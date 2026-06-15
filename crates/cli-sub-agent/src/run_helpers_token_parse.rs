//! Token usage and cost parsing from tool output.

use csa_session::TokenUsage;

/// Parse token usage from tool output (best-effort, returns None on failure).
///
/// Looks for common patterns in stdout/stderr:
/// - "tokens: N" or "Tokens: N" or "total_tokens: N"
/// - "input_tokens: N" / "output_tokens: N"
/// - "cache_read_input_tokens: N" or "cached_input_tokens: N"
/// - "reasoning_output_tokens: N" when provider output includes it
/// - "cost: $N.NN" or "estimated_cost: $N.NN"
pub(crate) fn parse_token_usage(output: &str) -> Option<TokenUsage> {
    let mut usage = TokenUsage::default();
    let mut found_any = false;

    // Simple pattern matching without regex
    for line in output.lines() {
        let line_lower = line.to_lowercase();

        found_any |= parse_usage_json_line(line, &mut usage);

        if let Some(val) = extract_key_number(line, &line_lower, "cache_read_input_tokens")
            .or_else(|| extract_key_number(line, &line_lower, "cached_input_tokens"))
        {
            usage.cache_read_input_tokens = Some(val);
            found_any = true;
        }

        if let Some(val) = extract_key_number(line, &line_lower, "reasoning_output_tokens")
            .or_else(|| extract_key_number(line, &line_lower, "reasoning_tokens"))
        {
            usage.reasoning_output_tokens = Some(val);
            found_any = true;
        }

        if let Some(val) = extract_key_number(line, &line_lower, "input_tokens") {
            usage.input_tokens = Some(val);
            found_any = true;
        }

        if let Some(val) = extract_key_number(line, &line_lower, "output_tokens") {
            usage.output_tokens = Some(val);
            found_any = true;
        }

        if let Some(val) = extract_key_number(line, &line_lower, "total_tokens") {
            usage.total_tokens = Some(val);
            found_any = true;
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

fn parse_usage_json_line(line: &str, usage: &mut TokenUsage) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return false;
    };
    let usage_value = value.get("usage").unwrap_or(&value);
    let mut found_any = false;

    if let Some(value) = json_u64_any(usage_value, &[&["input_tokens"], &["prompt_tokens"]]) {
        usage.input_tokens = Some(value);
        found_any = true;
    }
    if let Some(value) = json_u64_any(usage_value, &[&["output_tokens"], &["completion_tokens"]]) {
        usage.output_tokens = Some(value);
        found_any = true;
    }
    if let Some(value) = json_u64_any(usage_value, &[&["total_tokens"]]) {
        usage.total_tokens = Some(value);
        found_any = true;
    }
    if let Some(value) = json_u64_any(
        usage_value,
        &[
            &["cache_read_input_tokens"],
            &["cached_input_tokens"],
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
        ],
    ) {
        usage.cache_read_input_tokens = Some(value);
        found_any = true;
    }
    if let Some(value) = json_u64_any(
        usage_value,
        &[
            &["reasoning_output_tokens"],
            &["output_tokens_details", "reasoning_tokens"],
            &["completion_tokens_details", "reasoning_tokens"],
            &["reasoning_tokens"],
        ],
    ) {
        usage.reasoning_output_tokens = Some(value);
        found_any = true;
    }

    found_any
}

fn json_u64_any(value: &serde_json::Value, paths: &[&[&str]]) -> Option<u64> {
    paths.iter().find_map(|path| json_u64_path(value, path))
}

fn json_u64_path(value: &serde_json::Value, path: &[&str]) -> Option<u64> {
    let mut cursor = value;
    for segment in path {
        cursor = cursor.get(*segment)?;
    }
    cursor.as_u64()
}

fn extract_key_number(line: &str, line_lower: &str, key: &str) -> Option<u64> {
    let mut search_start = 0;
    while let Some(relative_pos) = line_lower[search_start..].find(key) {
        let pos = search_start + relative_pos;
        let key_end = pos + key.len();
        if is_key_boundary(line_lower.as_bytes(), pos, key_end) {
            return extract_number(&line[pos..]);
        }
        search_start = key_end;
    }
    None
}

fn is_key_boundary(bytes: &[u8], start: usize, end: usize) -> bool {
    let valid_prev = start == 0 || !is_identifier_byte(bytes[start - 1]);
    let valid_next = bytes.get(end).is_none_or(|next| !is_identifier_byte(*next));
    valid_prev && valid_next
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Extract a number after colon or equals sign.
pub(crate) fn extract_number(text: &str) -> Option<u64> {
    let colon = text.find(':');
    let equals = text.find('=');
    let start = match (colon, equals) {
        (Some(colon), Some(equals)) => colon.min(equals),
        (Some(colon), None) => colon,
        (None, Some(equals)) => equals,
        (None, None) => return None,
    };
    let after_separator = &text[start + 1..];

    // Take first word after colon
    let num_str: String = after_separator
        .chars()
        .skip_while(|c| c.is_whitespace() || *c == '"')
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
