use std::path::{Path, PathBuf};

/// A single stale reference to a removed skill found in a project file.
#[derive(Debug, Clone)]
pub struct StaleReference {
    pub file: PathBuf,
    pub line: usize,
    pub skill_name: String,
    pub context: String,
}

/// Scan project files for references to skill names that were removed
/// during upgrade. Returns an empty Vec if no stale references are found.
pub fn scan_stale_skill_references(
    project_root: &Path,
    removed_names: &[String],
) -> Vec<StaleReference> {
    if removed_names.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    scan_settings_json(project_root, removed_names, &mut results);
    // Scan markdown files that commonly reference skills.
    for md_path in &["CLAUDE.md", "AGENTS.md", ".claude/rules/AGENTS.md"] {
        scan_markdown(
            project_root,
            Path::new(md_path),
            removed_names,
            &mut results,
        );
    }

    results
}

/// Scan a settings.local.json file for `Skill(<name>)` or `Skill(<name>:*)`
/// patterns, but NOT Bash command history entries.
fn scan_settings_json(
    project_root: &Path,
    removed_names: &[String],
    out: &mut Vec<StaleReference>,
) {
    let rel = PathBuf::from(".claude/settings.local.json");
    let content = match std::fs::read_to_string(project_root.join(&rel)) {
        Ok(c) => c,
        Err(_) => return,
    };

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;

        for name in removed_names {
            // Match Skill(<name>) or Skill(<name>:anything).
            // The Skill() prefix is specific enough to distinguish from Bash
            // command history entries, so no line-level filtering is needed.
            let pattern_exact = format!("Skill({name})");
            let pattern_prefix = format!("Skill({name}:");

            if line.contains(&pattern_exact) || line.contains(&pattern_prefix) {
                out.push(StaleReference {
                    file: rel.clone(),
                    line: line_num,
                    skill_name: name.clone(),
                    context: line.trim().to_string(),
                });
            }
        }
    }
}

/// Scan a markdown file for `/<name>` (slash-prefixed) and word-boundary
/// references to removed skill names.
fn scan_markdown(
    project_root: &Path,
    rel: &Path,
    removed_names: &[String],
    out: &mut Vec<StaleReference>,
) {
    let content = match std::fs::read_to_string(project_root.join(rel)) {
        Ok(c) => c,
        Err(_) => return,
    };

    let rel = rel.to_path_buf();

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;

        for name in removed_names {
            let slash_pattern = format!("/{name}");

            // Check for /<name> reference (slash-prefixed)
            if let Some(pos) = line.find(&slash_pattern) {
                // Verify the slash is at a word boundary (start of line or preceded by
                // whitespace/punctuation)
                let before_ok = pos == 0 || {
                    let prev = line.as_bytes()[pos - 1];
                    prev.is_ascii_whitespace() || prev == b'(' || prev == b'`'
                };
                // After the pattern should be word boundary
                let after_pos = pos + slash_pattern.len();
                let after_ok = after_pos >= line.len() || {
                    let next = line.as_bytes()[after_pos];
                    next.is_ascii_whitespace()
                        || next == b')'
                        || next == b'`'
                        || next == b','
                        || next == b'.'
                        || next == b':'
                        || next == b';'
                };

                if before_ok && after_ok {
                    out.push(StaleReference {
                        file: rel.clone(),
                        line: line_num,
                        skill_name: name.clone(),
                        context: line.trim().to_string(),
                    });
                    continue; // Don't double-count this line for the same name
                }
            }

            // Check for word-boundary <name> reference.
            // Treat alphanumeric, underscore, AND hyphen as word characters
            // since skill names commonly use hyphens (e.g., "pr-codex-bot").
            // Without this, "commit" would false-positive on "ai-reviewed-commit".
            // Iterate all occurrences on the line so a rejected superset match
            // (e.g., "ai-reviewed-commit") doesn't mask a later standalone match.
            let mut search_start = 0;
            while let Some(offset) = line[search_start..].find(name.as_str()) {
                let pos = search_start + offset;
                let before_ok = pos == 0 || {
                    let prev = line.as_bytes()[pos - 1];
                    !prev.is_ascii_alphanumeric() && prev != b'_' && prev != b'-'
                };
                let after_pos = pos + name.len();
                let after_ok = after_pos >= line.len() || {
                    let next = line.as_bytes()[after_pos];
                    !next.is_ascii_alphanumeric() && next != b'_' && next != b'-'
                };

                if before_ok && after_ok {
                    out.push(StaleReference {
                        file: rel.clone(),
                        line: line_num,
                        skill_name: name.clone(),
                        context: line.trim().to_string(),
                    });
                    break; // One match per name per line is enough
                }
                search_start = pos + 1;
            }
        }
    }
}

#[cfg(test)]
#[path = "stale_ref_tests.rs"]
mod tests;
