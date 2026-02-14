//! Project context loader for auto-injecting CLAUDE.md/AGENTS.md into tool sessions.
//!
//! Loads project context files lazily (only when spawning a tool) and formats them
//! as tagged blocks for injection into the tool's prompt or system prompt.

use std::path::{Path, PathBuf};

use tracing::warn;

/// Default maximum total size of injected context (bytes).
const DEFAULT_MAX_CONTEXT_BYTES: usize = 50 * 1024;

/// A loaded context file with its relative path and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    /// Relative path from project root (e.g., "CLAUDE.md").
    pub rel_path: String,
    /// File content.
    pub content: String,
}

/// Options for context loading.
#[derive(Debug, Clone, Default)]
pub struct ContextLoadOptions {
    /// Files to skip loading (matched against relative path).
    pub skip_files: Vec<String>,
    /// Maximum total bytes of injected context. Defaults to 50KB.
    pub max_bytes: Option<usize>,
}

/// Load project context files from `project_root`.
///
/// Reads CLAUDE.md, AGENTS.md, and any detail files referenced by AGENTS.md
/// (lines matching `→ path/to/file.md`). Skips files listed in `options.skip_files`.
/// Missing files emit a warning but do not cause failure.
///
/// Total loaded content is capped at `options.max_bytes` (default 50KB).
pub fn load_project_context(project_root: &Path, options: &ContextLoadOptions) -> Vec<ContextFile> {
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_CONTEXT_BYTES);
    let mut files = Vec::new();
    let mut total_bytes: usize = 0;

    // Primary context files in priority order.
    let primary_files = ["CLAUDE.md", "AGENTS.md"];

    for rel_path in &primary_files {
        if options.skip_files.iter().any(|s| s == rel_path) {
            continue;
        }
        if let Some(cf) = try_load_file(project_root, rel_path, max_bytes, &mut total_bytes) {
            files.push(cf);
        }
    }

    // Parse AGENTS.md for detail file references if it was loaded.
    let agents_content = files
        .iter()
        .find(|f| f.rel_path == "AGENTS.md")
        .map(|f| f.content.clone());

    if let Some(content) = agents_content {
        let refs = parse_agents_references(&content);
        for ref_path in refs {
            let rel = ref_path.to_string_lossy().to_string();
            if options.skip_files.iter().any(|s| s == &rel) {
                continue;
            }
            if let Some(cf) = try_load_file(project_root, &rel, max_bytes, &mut total_bytes) {
                files.push(cf);
            }
        }
    }

    files
}

/// Format loaded context files as tagged blocks for prompt injection.
///
/// Format: `<context-file path="CLAUDE.md">\n{content}\n</context-file>`
pub fn format_context_for_prompt(files: &[ContextFile]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for cf in files {
        output.push_str(&format!(
            "<context-file path=\"{}\">\n{}\n</context-file>\n\n",
            cf.rel_path, cf.content
        ));
    }
    output
}

/// Try to load a single file, respecting the byte budget.
///
/// Validates that the resolved path stays within `project_root` to prevent
/// path traversal via `../` in AGENTS.md detail references.
fn try_load_file(
    project_root: &Path,
    rel_path: &str,
    max_bytes: usize,
    total_bytes: &mut usize,
) -> Option<ContextFile> {
    let full_path = project_root.join(rel_path);

    // Boundary check: canonicalize both paths and ensure the file is within
    // project_root. This prevents `../` traversal from escaping the project.
    let canonical_root = match project_root.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            warn!(path = %project_root.display(), error = %e, "Cannot canonicalize project root");
            return None;
        }
    };
    let canonical_file = match full_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %rel_path, error = %e, "Cannot canonicalize context file path");
            }
            return None;
        }
    };
    if !canonical_file.starts_with(&canonical_root) {
        warn!(
            path = %rel_path,
            resolved = %canonical_file.display(),
            root = %canonical_root.display(),
            "Context file path escapes project root (path traversal blocked)"
        );
        return None;
    }

    match std::fs::read_to_string(&full_path) {
        Ok(content) => {
            let new_total = *total_bytes + content.len();
            if new_total > max_bytes {
                warn!(
                    path = %rel_path,
                    file_bytes = content.len(),
                    total_so_far = *total_bytes,
                    max_bytes,
                    "Skipping context file: would exceed max context bytes"
                );
                return None;
            }
            *total_bytes = new_total;
            Some(ContextFile {
                rel_path: rel_path.to_string(),
                content,
            })
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %rel_path, error = %e, "Failed to read context file");
            }
            None
        }
    }
}

/// Parse AGENTS.md content for detail file references.
///
/// Looks for lines containing `→ path/to/file.md` patterns.
/// Handles both `→ ~/path` (home-relative, resolved to absolute) and
/// `→ path/to/file.md` (project-relative).
/// Returns only project-relative paths (skips home-relative ones since
/// those are user-private and may not exist in project root).
pub(crate) fn parse_agents_references(content: &str) -> Vec<PathBuf> {
    let mut refs = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Match lines like: `→ ~/s/llm/coding/rules/meta/001-task-delegation.md`
        // or `→ path/to/detail.md`
        if let Some(rest) = trimmed
            .strip_prefix("→")
            .or_else(|| trimmed.strip_prefix("->"))
        {
            let path_str = rest.trim().trim_matches('`');
            if path_str.is_empty() {
                continue;
            }
            // Skip home-relative paths (~/...) - these are user-private.
            if path_str.starts_with("~/") || path_str.starts_with('~') {
                continue;
            }
            // Skip absolute paths.
            if path_str.starts_with('/') {
                continue;
            }
            refs.push(PathBuf::from(path_str));
        }
    }

    refs
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_load_project_context_reads_claude_and_agents() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "# Project rules").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Agent rules").unwrap();

        let files = load_project_context(dir.path(), &ContextLoadOptions::default());
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].rel_path, "CLAUDE.md");
        assert_eq!(files[0].content, "# Project rules");
        assert_eq!(files[1].rel_path, "AGENTS.md");
        assert_eq!(files[1].content, "# Agent rules");
    }

    #[test]
    fn test_load_project_context_missing_files_no_error() {
        let dir = TempDir::new().unwrap();
        let files = load_project_context(dir.path(), &ContextLoadOptions::default());
        assert!(files.is_empty());
    }

    #[test]
    fn test_load_project_context_follows_agents_references() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "# Rules").unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "**001** `complexity`\n→ rules/001-complexity.md\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("rules")).unwrap();
        fs::write(
            dir.path().join("rules/001-complexity.md"),
            "# Complexity rules",
        )
        .unwrap();

        let files = load_project_context(dir.path(), &ContextLoadOptions::default());
        assert_eq!(files.len(), 3);
        assert_eq!(files[2].rel_path, "rules/001-complexity.md");
        assert_eq!(files[2].content, "# Complexity rules");
    }

    #[test]
    fn test_load_project_context_skips_home_relative_refs() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "→ ~/s/llm/coding/rules/meta/001.md\n→ rules/local.md\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("rules")).unwrap();
        fs::write(dir.path().join("rules/local.md"), "local content").unwrap();

        let files = load_project_context(dir.path(), &ContextLoadOptions::default());
        // AGENTS.md + rules/local.md (home-relative skipped)
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].rel_path, "rules/local.md");
    }

    #[test]
    fn test_load_project_context_respects_skip_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "# Rules").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Agents").unwrap();

        let options = ContextLoadOptions {
            skip_files: vec!["AGENTS.md".to_string()],
            ..Default::default()
        };
        let files = load_project_context(dir.path(), &options);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, "CLAUDE.md");
    }

    #[test]
    fn test_load_project_context_respects_max_bytes() {
        let dir = TempDir::new().unwrap();
        // Create a large CLAUDE.md that exceeds 100 bytes limit
        let big_content = "x".repeat(80);
        fs::write(dir.path().join("CLAUDE.md"), &big_content).unwrap();
        fs::write(dir.path().join("AGENTS.md"), "y".repeat(80)).unwrap();

        let options = ContextLoadOptions {
            max_bytes: Some(100),
            ..Default::default()
        };
        let files = load_project_context(dir.path(), &options);
        // Only CLAUDE.md fits (80 bytes), AGENTS.md (80 more) would exceed 100
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, "CLAUDE.md");
    }

    #[test]
    fn test_parse_agents_references_extracts_project_relative() {
        let content = "\
**001** `complexity` — Zero tolerance.\n\
→ rules/001-complexity.md\n\
\n\
**002** `strategic` — Allocate time.\n\
→ ~/s/llm/coding/rules/all-lang/002-strategic.md\n\
\n\
**003** `deep-modules`\n\
→ rules/003-deep-modules.md\n";

        let refs = parse_agents_references(content);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], PathBuf::from("rules/001-complexity.md"));
        assert_eq!(refs[1], PathBuf::from("rules/003-deep-modules.md"));
    }

    #[test]
    fn test_parse_agents_references_handles_arrow_variants() {
        let content = "→ rules/a.md\n-> rules/b.md\n→ `rules/c.md`\n";
        let refs = parse_agents_references(content);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0], PathBuf::from("rules/a.md"));
        assert_eq!(refs[1], PathBuf::from("rules/b.md"));
        assert_eq!(refs[2], PathBuf::from("rules/c.md"));
    }

    #[test]
    fn test_parse_agents_references_skips_absolute_paths() {
        let content = "→ /etc/passwd\n→ rules/ok.md\n";
        let refs = parse_agents_references(content);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], PathBuf::from("rules/ok.md"));
    }

    #[test]
    fn test_format_context_for_prompt_empty() {
        assert_eq!(format_context_for_prompt(&[]), "");
    }

    #[test]
    fn test_format_context_for_prompt_wraps_files() {
        let files = vec![
            ContextFile {
                rel_path: "CLAUDE.md".to_string(),
                content: "# Rules".to_string(),
            },
            ContextFile {
                rel_path: "AGENTS.md".to_string(),
                content: "# Agents".to_string(),
            },
        ];
        let formatted = format_context_for_prompt(&files);
        assert!(formatted.contains("<context-file path=\"CLAUDE.md\">"));
        assert!(formatted.contains("# Rules"));
        assert!(formatted.contains("</context-file>"));
        assert!(formatted.contains("<context-file path=\"AGENTS.md\">"));
    }

    #[test]
    fn test_load_project_context_blocks_path_traversal() {
        let dir = TempDir::new().unwrap();
        // Create a secret file outside the project root.
        let secret_path = dir.path().join("secret.txt");
        fs::write(&secret_path, "TOP SECRET").unwrap();

        // Create project as a subdirectory.
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("AGENTS.md"),
            "→ ../secret.txt\n→ rules/ok.md\n",
        )
        .unwrap();
        fs::create_dir_all(project.join("rules")).unwrap();
        fs::write(project.join("rules/ok.md"), "ok content").unwrap();

        let files = load_project_context(&project, &ContextLoadOptions::default());
        // AGENTS.md + rules/ok.md only; ../secret.txt blocked by boundary check.
        assert_eq!(files.len(), 2);
        let loaded_paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(!loaded_paths.contains(&"../secret.txt"));
        assert!(loaded_paths.contains(&"rules/ok.md"));
    }

    #[test]
    fn test_parse_agents_references_preserves_dotdot_for_boundary_check() {
        // Parser should NOT filter ../paths — that's try_load_file's job.
        let content = "→ ../escape.md\n→ rules/ok.md\n";
        let refs = parse_agents_references(content);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], PathBuf::from("../escape.md"));
        assert_eq!(refs[1], PathBuf::from("rules/ok.md"));
    }

    #[test]
    fn test_load_project_context_missing_ref_warns_but_continues() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "→ rules/nonexistent.md\n→ rules/exists.md\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("rules")).unwrap();
        fs::write(dir.path().join("rules/exists.md"), "content").unwrap();

        let files = load_project_context(dir.path(), &ContextLoadOptions::default());
        // AGENTS.md + rules/exists.md (nonexistent silently skipped)
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].rel_path, "rules/exists.md");
    }
}
