use csa_session::TokenUsage;

pub(crate) fn display_total_tokens(usage: &TokenUsage) -> Option<u64> {
    match (usage.input_tokens, usage.output_tokens) {
        (Some(input), Some(output)) => input.checked_add(output).or(usage.total_tokens),
        _ => usage.total_tokens,
    }
}

pub(crate) fn compact_token_usage(usage: Option<&TokenUsage>) -> String {
    usage
        .and_then(compact_token_usage_inner)
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn token_usage_json_value(usage: &TokenUsage) -> serde_json::Value {
    let mut value = match serde_json::to_value(usage) {
        Ok(value) => value,
        Err(_) => serde_json::Value::Null,
    };
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if let Some(total) = display_total_tokens(usage) {
        object.insert("total_tokens".to_string(), serde_json::json!(total));
    }
    if let Some(uncached) = usage.uncached_input_tokens() {
        object.insert(
            "uncached_input_tokens".to_string(),
            serde_json::json!(uncached),
        );
    }
    if let Some(ratio) = usage.cache_read_ratio() {
        object.insert("cache_read_ratio".to_string(), serde_json::json!(ratio));
        object.insert("cache_hit_ratio".to_string(), serde_json::json!(ratio));
    }
    value
}

fn compact_token_usage_inner(usage: &TokenUsage) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(total) = display_total_tokens(usage) {
        parts.push(format!("{total}tok"));
    }
    if let Some(uncached) = usage.uncached_input_tokens() {
        parts.push(format!("uncached={uncached}"));
    }
    if let Some(ratio) = usage.cache_read_ratio() {
        parts.push(format!("cache={:.0}%", ratio * 100.0));
    } else if let Some(cache_read) = usage.cache_read_input_tokens {
        parts.push(format!("cache_read={cache_read}"));
    }
    if let Some(cost) = usage.estimated_cost_usd {
        parts.push(format!("${cost:.4}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_token_usage_includes_uncached_and_cache_ratio() {
        let usage = TokenUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(250),
            reasoning_output_tokens: Some(100),
            total_tokens: None,
            estimated_cost_usd: None,
            cache_read_input_tokens: Some(750),
        };

        assert_eq!(
            compact_token_usage(Some(&usage)),
            "1250tok uncached=250 cache=75%"
        );
    }

    #[test]
    fn token_usage_json_value_adds_derived_fields_without_zeroing_missing_fields() {
        let usage = TokenUsage {
            input_tokens: Some(1_000),
            output_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: None,
            estimated_cost_usd: None,
            cache_read_input_tokens: Some(600),
        };

        let value = token_usage_json_value(&usage);

        assert_eq!(value["input_tokens"], 1_000);
        assert_eq!(value.get("output_tokens"), None);
        assert_eq!(value["uncached_input_tokens"], 400);
        assert_eq!(value["cache_read_ratio"], serde_json::json!(0.6));
    }
}
