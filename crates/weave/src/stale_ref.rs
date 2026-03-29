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

/// Scan a markdown file for precise skill-reference patterns:
/// - `/<name>` — slash-prefixed skill invocation
/// - `` `<name>` `` — backtick-quoted code span
/// - `Skill(<name>)` — Claude Code Skill() pattern in markdown
///
/// Bare word-boundary matches are intentionally excluded because skill
/// names like `commit` or `csa` are also common English words, producing
/// excessive false positives in documentation.
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
            // Pattern 1: /<name> (slash-prefixed skill invocation)
            // Iterate all occurrences so a URL like .com/name doesn't mask
            // a real /name invocation later on the same line.
            let slash_pattern = format!("/{name}");
            let mut slash_found = false;
            let mut slash_start = 0;
            while let Some(offset) = line[slash_start..].find(&slash_pattern) {
                let pos = slash_start + offset;
                let before_ok = pos == 0 || {
                    let prev = line.as_bytes()[pos - 1];
                    prev.is_ascii_whitespace() || prev == b'(' || prev == b'`'
                };
                let after_pos = pos + slash_pattern.len();
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
                    slash_found = true;
                    break;
                }
                slash_start = pos + 1;
            }
            if slash_found {
                continue;
            }

            // Pattern 2: `<name>` (backtick-quoted code span)
            let backtick_pattern = format!("`{name}`");
            if line.contains(&backtick_pattern) {
                out.push(StaleReference {
                    file: rel.clone(),
                    line: line_num,
                    skill_name: name.clone(),
                    context: line.trim().to_string(),
                });
                continue;
            }

            // Pattern 3: Skill(<name>) (Claude Code pattern in markdown)
            let skill_pattern = format!("Skill({name})");
            let skill_prefix = format!("Skill({name}:");
            if line.contains(&skill_pattern) || line.contains(&skill_prefix) {
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

#[cfg(test)]
#[path = "stale_ref_tests.rs"]
mod tests;
