pub(super) fn suggest_key_paths(
    key: &str,
    candidates: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let query = key.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    let last_segment = query.rsplit('.').next().unwrap_or(query.as_str());
    let max_distance = std::cmp::max(3, query.len() / 3);

    let mut ranked: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| {
            if candidate == key {
                return None;
            }

            let normalized = candidate.to_ascii_lowercase();
            let full_prefix = normalized.starts_with(&query);
            let segment_prefix = normalized
                .rsplit('.')
                .next()
                .is_some_and(|segment| segment.starts_with(last_segment));
            let contains = normalized.contains(&query)
                || (last_segment.len() >= 3 && normalized.contains(last_segment));
            let distance = levenshtein_distance(&query, &normalized);

            (full_prefix || segment_prefix || contains || distance <= max_distance).then(|| {
                (
                    !full_prefix,
                    !segment_prefix,
                    !contains,
                    distance,
                    normalized.len().abs_diff(query.len()),
                    candidate.clone(),
                )
            })
        })
        .collect();

    ranked.sort();
    ranked
        .into_iter()
        .map(|(_, _, _, _, _, candidate)| candidate)
        .take(5)
        .collect()
}

fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_chars: Vec<_> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_idx, left_ch) in left.chars().enumerate() {
        current[0] = left_idx + 1;
        for (right_idx, right_ch) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_ch != *right_ch);
            current[right_idx + 1] = std::cmp::min(
                std::cmp::min(previous[right_idx + 1] + 1, current[right_idx] + 1),
                previous[right_idx] + substitution_cost,
            );
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right_chars.len()]
}

pub(super) fn format_missing_key_message(key: &str, suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        return format!("Key not found: {key}");
    }

    let suggestion_lines = suggestions
        .iter()
        .map(|candidate| format!("  - {candidate}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Key not found: {key}\nClosest matches:\n{suggestion_lines}")
}

/// Navigate a TOML value by dotted key path (e.g., "tools.codex.enabled").
pub(super) fn resolve_key(root: &toml::Value, key: &str) -> Option<toml::Value> {
    let mut current = root;
    for part in key.split('.') {
        current = current.as_table()?.get(part)?;
    }
    Some(current.clone())
}

/// Format a TOML value for stdout (inline for scalars, pretty for tables/arrays).
pub(super) fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Table(_) | toml::Value::Array(_) => {
            toml::to_string_pretty(value).unwrap_or_else(|_| format!("{value:?}"))
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}
