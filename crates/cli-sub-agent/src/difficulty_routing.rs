use anyhow::Result;
use csa_config::ProjectConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPromptDifficulty {
    pub(crate) difficulty: Option<String>,
    pub(crate) prompt: String,
}

pub(crate) fn strip_difficulty_frontmatter(prompt: String) -> Result<ParsedPromptDifficulty> {
    let Some(body_start) = frontmatter_body_start(&prompt) else {
        return Ok(ParsedPromptDifficulty {
            difficulty: None,
            prompt,
        });
    };

    let mut line_start = body_start;
    for line in prompt[body_start..].split_inclusive('\n') {
        let line_end = line_start + line.len();
        if line.trim_end() == "---" {
            let frontmatter = &prompt[body_start..line_start];
            let difficulty = parse_frontmatter_difficulty(frontmatter)?;
            let mut prompt = prompt;
            prompt.drain(..line_end);
            return Ok(ParsedPromptDifficulty { difficulty, prompt });
        }
        line_start = line_end;
    }

    anyhow::bail!("Malformed YAML frontmatter: opening '---' has no closing '---' delimiter")
}

fn frontmatter_body_start(prompt: &str) -> Option<usize> {
    let first_line = prompt.split_inclusive('\n').next().unwrap_or(prompt);
    if first_line.trim_end() == "---" {
        Some(first_line.len())
    } else {
        None
    }
}

fn parse_frontmatter_difficulty(frontmatter: &str) -> Result<Option<String>> {
    let mut difficulty = None;
    for raw_line in frontmatter.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "difficulty" {
            continue;
        }
        let value = strip_yaml_trailing_comment(value.trim());
        let label = unquote_yaml_scalar(value);
        if label.trim().is_empty() {
            anyhow::bail!("Malformed YAML frontmatter: difficulty value cannot be empty");
        }
        difficulty = Some(label.to_string());
    }
    Ok(difficulty)
}

/// Strip a trailing `# comment` from a YAML scalar value, skipping `#` inside
/// single- or double-quoted regions. Returns the trimmed value slice.
fn strip_yaml_trailing_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                // skip closing quote
                i += 1;
            }
            b'#' => {
                return value[..i].trim_end();
            }
            _ => {
                i += 1;
            }
        }
    }
    value
}

fn unquote_yaml_scalar(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let quoted = (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'');
        if quoted {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub(crate) fn resolve_effective_tier_with_difficulty_hint(
    config: Option<&ProjectConfig>,
    explicit_tier: Option<&str>,
    model_spec: Option<&str>,
    cli_hint: Option<&str>,
    frontmatter_hint: Option<&str>,
) -> Result<Option<String>> {
    if let Some(tier) = explicit_tier {
        return Ok(Some(tier.to_string()));
    }
    if model_spec.is_some() {
        return Ok(None);
    }
    let Some(label) = cli_hint.or(frontmatter_hint) else {
        return Ok(None);
    };
    resolve_tier_mapping_label(config, label).map(Some)
}

pub(crate) fn resolve_tier_mapping_label(
    config: Option<&ProjectConfig>,
    label: &str,
) -> Result<String> {
    let normalized = label.trim();
    if normalized.is_empty() {
        anyhow::bail!(
            "Difficulty hint label cannot be empty. Available difficulty labels: [{}]",
            available_difficulty_labels(config)
        );
    }

    let Some(cfg) = config else {
        anyhow::bail!(
            "Difficulty hint '{}' requires [tier_mapping], but no project config is loaded.",
            normalized
        );
    };

    let Some(tier_name) = cfg.tier_mapping.get(normalized) else {
        anyhow::bail!(
            "Difficulty hint '{}' not found in [tier_mapping]. Available difficulty labels: [{}]",
            normalized,
            available_difficulty_labels(Some(cfg))
        );
    };

    if !cfg.tiers.contains_key(tier_name) {
        let mut available_tiers: Vec<&str> = cfg.tiers.keys().map(String::as_str).collect();
        available_tiers.sort_unstable();
        anyhow::bail!(
            "tier_mapping.{} references unknown tier '{}'. Available tiers: [{}]",
            normalized,
            tier_name,
            available_tiers.join(", ")
        );
    }

    Ok(tier_name.clone())
}

fn available_difficulty_labels(config: Option<&ProjectConfig>) -> String {
    let Some(cfg) = config else {
        return "none".to_string();
    };
    if cfg.tier_mapping.is_empty() {
        return "none".to_string();
    }
    let mut labels: Vec<&str> = cfg.tier_mapping.keys().map(String::as_str).collect();
    labels.sort_unstable();
    labels.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, TierConfig, TierStrategy, ToolConfig};
    use std::collections::HashMap;

    fn test_config() -> ProjectConfig {
        let mut tiers = HashMap::new();
        tiers.insert(
            "tier-1-quick".to_string(),
            TierConfig {
                description: "Quick".to_string(),
                models: vec!["claude-code/anthropic/default/low".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );
        tiers.insert(
            "tier-2-standard".to_string(),
            TierConfig {
                description: "Standard".to_string(),
                models: vec!["claude-code/anthropic/default/high".to_string()],
                strategy: TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );

        ProjectConfig {
            schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: chrono::Utc::now(),
                max_recursion_depth: 5,
            },
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools: HashMap::<String, ToolConfig>::new(),
            review: None,
            debate: None,
            tiers,
            tier_mapping: HashMap::from([
                ("bug_fix".to_string(), "tier-2-standard".to_string()),
                ("quick_question".to_string(), "tier-1-quick".to_string()),
            ]),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            run: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    #[test]
    fn frontmatter_parse_valid_difficulty_and_strips_block() {
        let parsed = strip_difficulty_frontmatter(
            "---\ndifficulty: quick_question\n---\nanswer this".to_string(),
        )
        .expect("valid frontmatter");

        assert_eq!(parsed.difficulty.as_deref(), Some("quick_question"));
        assert_eq!(parsed.prompt, "answer this");
    }

    #[test]
    fn frontmatter_parse_missing_difficulty_strips_block() {
        let parsed = strip_difficulty_frontmatter("---\nowner: docs\n---\nbody".to_string())
            .expect("frontmatter without difficulty is valid");

        assert_eq!(parsed.difficulty, None);
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn frontmatter_parse_ignores_yaml_sequence_lines() {
        let parsed = strip_difficulty_frontmatter(
            "---\ntags:\n- foo\n- bar\ndifficulty: quick_question\n---\nbody".to_string(),
        )
        .expect("sequence frontmatter should be accepted");

        assert_eq!(parsed.difficulty.as_deref(), Some("quick_question"));
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn frontmatter_parse_ignores_unknown_lines() {
        let parsed = strip_difficulty_frontmatter(
            "---\nowner: docs\nfreeform owner line\ndifficulty: bug_fix\n---\nbody".to_string(),
        )
        .expect("unknown frontmatter lines should be accepted");

        assert_eq!(parsed.difficulty.as_deref(), Some("bug_fix"));
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn frontmatter_parse_allows_delimiter_trailing_space_and_crlf() {
        let parsed = strip_difficulty_frontmatter(
            "---  \r\ndifficulty: quick_question\r\n--- \t\r\nbody".to_string(),
        )
        .expect("frontmatter delimiters may have trailing whitespace");

        assert_eq!(parsed.difficulty.as_deref(), Some("quick_question"));
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn frontmatter_parse_empty_difficulty_errors() {
        let err = strip_difficulty_frontmatter("---\ndifficulty:\n---\nbody".to_string())
            .expect_err("empty difficulty must error");

        assert!(
            err.to_string().contains("difficulty value cannot be empty"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn no_frontmatter_passes_prompt_through() {
        let parsed = strip_difficulty_frontmatter("plain prompt".to_string())
            .expect("plain prompt should pass through");

        assert_eq!(parsed.difficulty, None);
        assert_eq!(parsed.prompt, "plain prompt");
    }

    #[test]
    fn tier_mapping_lookup_hits() {
        let config = test_config();
        let tier = resolve_tier_mapping_label(Some(&config), "quick_question")
            .expect("quick_question should resolve");

        assert_eq!(tier, "tier-1-quick");
    }

    #[test]
    fn tier_mapping_lookup_miss_lists_available_labels() {
        let config = test_config();
        let err = resolve_tier_mapping_label(Some(&config), "security_audit")
            .expect_err("missing label must error");
        let msg = err.to_string();

        assert!(msg.contains("Difficulty hint 'security_audit' not found in [tier_mapping]"));
        assert!(msg.contains("bug_fix"));
        assert!(msg.contains("quick_question"));
    }

    #[test]
    fn strip_frontmatter_drains_in_place() {
        let original = "---\ndifficulty: bug_fix\n---\nthe body here".to_string();
        let original_ptr = original.as_ptr();
        let parsed = strip_difficulty_frontmatter(original).expect("valid frontmatter");

        assert_eq!(parsed.difficulty.as_deref(), Some("bug_fix"));
        assert_eq!(parsed.prompt, "the body here");
        // The returned string reuses the original allocation (drain in-place).
        assert_eq!(parsed.prompt.as_ptr(), original_ptr);
    }

    #[test]
    fn frontmatter_strips_trailing_yaml_comment() {
        let parsed = strip_difficulty_frontmatter(
            "---\ndifficulty: quick_question # 30%\n---\nbody".to_string(),
        )
        .expect("trailing comment should be stripped");

        assert_eq!(parsed.difficulty.as_deref(), Some("quick_question"));
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn frontmatter_preserves_hash_inside_quotes() {
        // Single-quoted value containing '#' must NOT be stripped.
        let parsed = strip_difficulty_frontmatter(
            "---\ndifficulty: 'quick#question'\n---\nbody".to_string(),
        )
        .expect("hash inside quotes must be preserved");
        assert_eq!(parsed.difficulty.as_deref(), Some("quick#question"));

        // Double-quoted value containing '#' must NOT be stripped.
        let parsed = strip_difficulty_frontmatter(
            "---\ndifficulty: \"quick#question\"\n---\nbody".to_string(),
        )
        .expect("hash inside double quotes must be preserved");
        assert_eq!(parsed.difficulty.as_deref(), Some("quick#question"));
    }

    #[test]
    fn frontmatter_quoted_value_with_trailing_comment() {
        let parsed = strip_difficulty_frontmatter(
            "---\ndifficulty: 'bug_fix' # a comment\n---\nbody".to_string(),
        )
        .expect("quoted value with trailing comment");
        assert_eq!(parsed.difficulty.as_deref(), Some("bug_fix"));
    }

    #[test]
    fn cli_hint_wins_over_frontmatter_hint() {
        let config = test_config();
        let tier = resolve_effective_tier_with_difficulty_hint(
            Some(&config),
            None,
            None,
            Some("bug_fix"),
            Some("quick_question"),
        )
        .expect("CLI hint should resolve")
        .expect("tier selected");

        assert_eq!(tier, "tier-2-standard");
    }
}
