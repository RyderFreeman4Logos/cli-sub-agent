use super::ProjectConfig;

impl ProjectConfig {
    /// Resolve the canonical tier name for a task type.
    ///
    /// Uses `tier_mapping[task_type]` first, then falls back to tier3-compatible
    /// names for legacy configs. The returned tier may still be absent if
    /// `tier_mapping` points at a missing tier; callers should look it up in
    /// `tiers` and treat absence as no match.
    pub fn resolve_tier_name_for_task(&self, task_type: &str) -> Option<&str> {
        if let Some(mapped) = self.tier_mapping.get(task_type) {
            return Some(mapped.as_str());
        }
        if self.tiers.contains_key("tier3") {
            return Some("tier3");
        }
        self.tiers
            .keys()
            .find(|k| k.starts_with("tier-3-") || k.starts_with("tier3"))
            .map(String::as_str)
    }

    /// Try parsing `selector` as a compound `<tier>-<tool>` form.
    ///
    /// Splits on `-` boundaries from the rightmost segment outward so that
    /// multi-hyphen tool names like `claude-code` are recognized after the
    /// single-segment `code` suffix fails to match. Returns the canonical
    /// tier name plus the parsed tool on the first split where the suffix is
    /// a recognized tool (canonical, built-in alias, or user-defined alias)
    /// AND the remaining prefix resolves to a configured tier via
    /// `resolve_tier_selector`.
    ///
    /// Used as a fallback after `resolve_tier_selector` returns `None` to
    /// support compound CLI forms like `--tier tier-4-critical-codex`.
    pub fn try_parse_compound_tier_tool(
        &self,
        selector: &str,
    ) -> Option<(String, csa_core::types::ToolName)> {
        use csa_core::types::ToolArg;
        use std::str::FromStr;

        let hyphen_positions: Vec<usize> = selector
            .char_indices()
            .filter(|(_, c)| *c == '-')
            .map(|(i, _)| i)
            .collect();
        if hyphen_positions.is_empty() {
            return None;
        }

        for &pos in hyphen_positions.iter().rev() {
            let prefix = &selector[..pos];
            let suffix = &selector[pos + 1..];
            if prefix.is_empty() || suffix.is_empty() {
                continue;
            }

            let Ok(parsed) = ToolArg::from_str(suffix) else {
                continue;
            };
            let tool = match parsed {
                ToolArg::Specific(t) => t,
                ToolArg::Alias(alias_name) => {
                    let Some(canonical) = self.tool_aliases.get(&alias_name) else {
                        continue;
                    };
                    match ToolArg::from_str(canonical) {
                        Ok(ToolArg::Specific(t)) => t,
                        _ => continue,
                    }
                }
                ToolArg::Auto | ToolArg::AnyAvailable => continue,
            };

            if let Some(canonical_tier) = self.resolve_tier_selector(prefix) {
                return Some((canonical_tier, tool));
            }
        }
        None
    }

    /// Suggest a tier name for a failed selector (for "Did you mean?" messages).
    ///
    /// Returns `Some(name)` when exactly one tier starts with the selector,
    /// or the selector is a substring of exactly one tier name.
    pub fn suggest_tier(&self, selector: &str) -> Option<String> {
        if selector.trim().is_empty() {
            return None;
        }
        if let Some(replacement) = legacy_tier_selector_replacement(selector)
            && self.tiers.contains_key(replacement)
        {
            return Some(replacement.to_string());
        }
        let prefix_matches: Vec<&String> = self
            .tiers
            .keys()
            .filter(|name| name.starts_with(selector))
            .collect();
        if prefix_matches.len() == 1 {
            return Some(prefix_matches[0].clone());
        }
        let substr_matches: Vec<&String> = self
            .tiers
            .keys()
            .filter(|name| name.contains(selector))
            .collect();
        if substr_matches.len() == 1 {
            return Some(substr_matches[0].clone());
        }
        None
    }

    /// Format tier aliases for error messages (empty string if no mappings).
    pub fn format_tier_aliases(&self) -> String {
        if self.tier_mapping.is_empty() {
            return String::new();
        }
        let mut aliases: Vec<String> = self
            .tier_mapping
            .iter()
            .map(|(k, v)| format!("{k} \u{2192} {v}"))
            .collect();
        aliases.sort();
        format!("\nAvailable tier aliases: [{}]", aliases.join(", "))
    }
}

fn legacy_tier_selector_replacement(selector: &str) -> Option<&'static str> {
    match selector {
        "tier-4-hard" | "tier4-hard" => Some("tier-4-critical"),
        _ => None,
    }
}
