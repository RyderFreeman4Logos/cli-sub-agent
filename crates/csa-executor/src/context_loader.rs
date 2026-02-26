//! Project context loader for auto-injecting CLAUDE.md/AGENTS.md into tool sessions.
//!
//! Loads project context files lazily (only when spawning a tool) and formats them
//! as tagged blocks for injection into the tool's prompt or system prompt.

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

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
        // Primary files may be symlinked to shared configs — allow external targets.
        if let Some(cf) = try_load_file(project_root, rel_path, max_bytes, &mut total_bytes, true) {
            files.push(cf);
        }
    }

    // Parse AGENTS.md for detail file references if it was loaded.
    // Collect refs first to avoid borrowing `files` during mutation.
    let detail_refs: Vec<PathBuf> = files
        .iter()
        .find(|f| f.rel_path == "AGENTS.md")
        .map(|f| parse_agents_references(&f.content))
        .unwrap_or_default();

    for ref_path in detail_refs {
        let rel = ref_path.to_string_lossy().to_string();
        if options.skip_files.iter().any(|s| s == &rel) {
            continue;
        }
        // Detail refs are repo-controlled input — symlinks MUST NOT escape project root.
        if let Some(cf) = try_load_file(project_root, &rel, max_bytes, &mut total_bytes, false) {
            files.push(cf);
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
///
/// `allow_external_symlink`: when `true`, symlinked files whose logical path
/// is within the project are allowed even if their target is outside (for
/// primary files like CLAUDE.md/AGENTS.md). When `false`, all symlinks must
/// resolve within the project root (for AGENTS detail refs).
fn try_load_file(
    project_root: &Path,
    rel_path: &str,
    max_bytes: usize,
    total_bytes: &mut usize,
    allow_external_symlink: bool,
) -> Option<ContextFile> {
    let full_path = project_root.join(rel_path);

    // Boundary check: prevent path traversal while allowing legitimate symlinks.
    //
    // Symlinked CLAUDE.md/AGENTS.md are common (users share configs across projects).
    // canonicalize() resolves symlinks to real paths, which then fail starts_with()
    // because the real path is outside project_root. Instead, we first check the
    // logical (un-resolved) path. If it stays within the project root lexically,
    // the symlink is intentional and allowed. Only fall back to canonicalize for
    // non-symlink paths where `../` components might escape the root.
    let canonical_root = match project_root.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            warn!(path = %project_root.display(), error = %e, "Cannot canonicalize project root");
            return None;
        }
    };

    // First check: does the logical (un-resolved) path stay within the project?
    // `full_path` is `project_root.join(rel_path)` — if rel_path has no `..`
    // escaping beyond the root, this passes. Symlinks pass here because their
    // logical location is within the project, even if the target is outside.
    let logical_ok =
        full_path.exists() && !rel_path.starts_with("..") && !rel_path.contains("/../");

    // For non-symlink files or as a secondary check, use canonicalize.
    // This catches tricky `../` traversal that lexical checks might miss.
    let boundary_ok = if full_path.is_symlink() && allow_external_symlink {
        // Primary file symlink: trust the logical path check. The user placed
        // this symlink inside the project directory intentionally.
        if logical_ok {
            debug!(
                path = %rel_path,
                target = %std::fs::read_link(&full_path).map(|p| p.display().to_string()).unwrap_or_default(),
                "Allowing symlinked primary context file"
            );
            true
        } else {
            warn!(
                path = %rel_path,
                "Symlinked context file has suspicious relative path (blocked)"
            );
            false
        }
    } else {
        // Non-symlink: canonicalize and verify it stays within the project root.
        match full_path.canonicalize() {
            Ok(canonical_file) => {
                if canonical_file.starts_with(&canonical_root) {
                    true
                } else {
                    warn!(
                        path = %rel_path,
                        resolved = %canonical_file.display(),
                        root = %canonical_root.display(),
                        "Context file path escapes project root (path traversal blocked)"
                    );
                    false
                }
            }
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(path = %rel_path, error = %e, "Cannot canonicalize context file path");
                }
                return None;
            }
        }
    };

    if !boundary_ok {
        return None;
    }

    // Pre-check file size via metadata before reading into memory.
    let file_size = match std::fs::metadata(&full_path) {
        Ok(meta) => meta.len() as usize,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %rel_path, error = %e, "Failed to read context file metadata");
            }
            return None;
        }
    };

    if let Some(new_total) = total_bytes.checked_add(file_size) {
        if new_total > max_bytes {
            warn!(
                path = %rel_path,
                file_bytes = file_size,
                total_so_far = *total_bytes,
                max_bytes,
                "Skipping context file: would exceed max context bytes"
            );
            return None;
        }
    } else {
        warn!(
            path = %rel_path,
            file_bytes = file_size,
            total_so_far = *total_bytes,
            "Skipping context file: byte count overflow"
        );
        return None;
    }

    match std::fs::read_to_string(&full_path) {
        Ok(content) => {
            // Re-check with actual content length (may differ from metadata for
            // multi-byte encodings or platform quirks, but use actual for accuracy).
            let actual_new_total = match total_bytes.checked_add(content.len()) {
                Some(t) if t <= max_bytes => t,
                Some(_) => {
                    warn!(
                        path = %rel_path,
                        file_bytes = content.len(),
                        total_so_far = *total_bytes,
                        max_bytes,
                        "Skipping context file: would exceed max context bytes"
                    );
                    return None;
                }
                None => {
                    warn!(
                        path = %rel_path,
                        "Skipping context file: byte count overflow"
                    );
                    return None;
                }
            };
            *total_bytes = actual_new_total;
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

/// Instructions appended to prompts when structured output is enabled.
///
/// Tells agents to wrap output in `<!-- CSA:SECTION:<id> -->` delimiters
/// so the output parser can extract machine-readable sections.
const STRUCTURED_OUTPUT_INSTRUCTIONS: &str = "\
\n\n<csa-output-format>\n\
Wrap your output in section markers for structured parsing:\n\
<!-- CSA:SECTION:summary -->\n\
Brief summary of result\n\
<!-- CSA:SECTION:summary:END -->\n\
\n\
<!-- CSA:SECTION:details -->\n\
Full analysis, code, or explanation\n\
<!-- CSA:SECTION:details:END -->\n\
</csa-output-format>";

/// Full structured-output instructions used in fork-call mode.
const FORK_CALL_STRUCTURED_OUTPUT_INSTRUCTIONS: &str = "\
\n\n<csa-output-format>\n\
Wrap your output in section markers for structured parsing:\n\
<!-- CSA:SECTION:summary -->\n\
Brief summary of result\n\
<!-- CSA:SECTION:summary:END -->\n\
\n\
<!-- CSA:SECTION:details -->\n\
Full analysis, code, or explanation\n\
<!-- CSA:SECTION:details:END -->\n\
</csa-output-format>\n\n<csa-fork-call-return>\n\
Fork-call mode requires a machine-readable return packet section.\n\
You MUST output this section exactly once using TOML:\n\
<!-- CSA:SECTION:return-packet -->\n\
status = \"Success\" # Success | Failure | Cancelled\n\
exit_code = 0\n\
summary = \"Short summary of completed work\"\n\
artifacts = [\"path/to/artifact\"]\n\
changed_files = [{ path = \"src/file.rs\", action = \"Modify\" }] # action: Add|Modify|Delete\n\
git_head_before = \"<optional commit sha>\"\n\
git_head_after = \"<optional commit sha>\"\n\
next_actions = [\"optional follow-up item\"]\n\
error_context = \"optional failure context\"\n\
<!-- CSA:SECTION:return-packet:END -->\n\
</csa-fork-call-return>";

/// Return the structured output instruction block for prompt injection.
///
/// Returns `Some(instructions)` when `enabled` is true, `None` otherwise.
/// The caller appends this to the effective prompt.
pub fn structured_output_instructions(enabled: bool) -> Option<&'static str> {
    if enabled {
        Some(STRUCTURED_OUTPUT_INSTRUCTIONS)
    } else {
        None
    }
}

/// Return fork-call structured output instructions.
///
/// The returned prompt includes both default structured-output requirements
/// and the fork-call-specific return-packet section schema.
pub fn structured_output_instructions_for_fork_call(enabled: bool) -> Option<&'static str> {
    if enabled {
        Some(FORK_CALL_STRUCTURED_OUTPUT_INSTRUCTIONS)
    } else {
        None
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

    #[cfg(unix)]
    #[test]
    fn test_load_project_context_follows_symlinked_claude_md() {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        // Create real file outside the project directory.
        let external = dir.path().join("shared-config");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("CLAUDE.md"), "# Shared rules").unwrap();

        // Create project directory with symlink to external CLAUDE.md.
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        unix_fs::symlink(external.join("CLAUDE.md"), project.join("CLAUDE.md")).unwrap();

        let files = load_project_context(&project, &ContextLoadOptions::default());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, "CLAUDE.md");
        assert_eq!(files[0].content, "# Shared rules");
    }

    #[cfg(unix)]
    #[test]
    fn test_load_project_context_follows_symlinked_agents_md() {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        // Create real AGENTS.md outside project.
        let external = dir.path().join("shared");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("AGENTS.md"), "# Shared agents").unwrap();

        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        unix_fs::symlink(external.join("AGENTS.md"), project.join("AGENTS.md")).unwrap();

        let files = load_project_context(&project, &ContextLoadOptions::default());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel_path, "AGENTS.md");
        assert_eq!(files[0].content, "# Shared agents");
    }

    #[cfg(unix)]
    #[test]
    fn test_load_project_context_blocks_dotdot_traversal_even_as_symlink() {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        let secret = dir.path().join("secret.txt");
        fs::write(&secret, "TOP SECRET").unwrap();

        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        // AGENTS.md references ../secret.txt — this should be blocked
        // because the rel_path starts with ".." (traversal attempt).
        fs::write(project.join("AGENTS.md"), "→ ../secret.txt\n").unwrap();
        // Also create a symlink with .. in its name to test the rel_path check.
        unix_fs::symlink(&secret, project.join("legit-link.txt")).unwrap();

        let files = load_project_context(&project, &ContextLoadOptions::default());
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(!paths.contains(&"../secret.txt"));
    }

    #[test]
    fn test_structured_output_instructions_enabled() {
        let result = structured_output_instructions(true);
        assert!(result.is_some());
        let instructions = result.unwrap();
        assert!(instructions.contains("CSA:SECTION:summary"));
        assert!(instructions.contains("CSA:SECTION:details"));
        assert!(instructions.contains("CSA:SECTION:summary:END"));
        assert!(instructions.contains("CSA:SECTION:details:END"));
        assert!(instructions.contains("<csa-output-format>"));
    }

    #[test]
    fn test_structured_output_instructions_disabled() {
        assert!(structured_output_instructions(false).is_none());
    }

    #[test]
    fn test_structured_output_instructions_token_budget() {
        // Instructions must be < 200 tokens (~150 words).
        let instructions = structured_output_instructions(true).unwrap();
        let word_count = instructions.split_whitespace().count();
        assert!(
            word_count < 150,
            "Structured output instructions too long: {word_count} words (max ~150)"
        );
    }

    #[test]
    fn test_structured_output_instructions_for_fork_call_enabled() {
        let instructions = structured_output_instructions_for_fork_call(true).unwrap();
        assert!(instructions.contains("CSA:SECTION:summary"));
        assert!(instructions.contains("CSA:SECTION:details"));
        assert!(instructions.contains("CSA:SECTION:return-packet"));
        assert!(instructions.contains("status = \"Success\""));
        assert!(instructions.contains("changed_files = [{ path ="));
    }

    #[test]
    fn test_structured_output_instructions_for_fork_call_disabled() {
        assert!(structured_output_instructions_for_fork_call(false).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_load_project_context_blocks_symlinked_detail_ref_outside_root() {
        use std::os::unix::fs as unix_fs;

        let dir = TempDir::new().unwrap();
        // Create a secret file outside the project.
        let secret = dir.path().join("secret.txt");
        fs::write(&secret, "TOP SECRET").unwrap();

        let project = dir.path().join("project");
        fs::create_dir_all(project.join("rules")).unwrap();
        // Create a symlink inside project that points to the secret file.
        unix_fs::symlink(&secret, project.join("rules/ext.md")).unwrap();
        // AGENTS.md references the symlink via detail ref.
        fs::write(project.join("AGENTS.md"), "→ rules/ext.md\n").unwrap();

        let files = load_project_context(&project, &ContextLoadOptions::default());
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        // Detail ref symlink pointing outside root MUST be blocked.
        assert!(!paths.contains(&"rules/ext.md"));
        // AGENTS.md itself should still load.
        assert!(paths.contains(&"AGENTS.md"));
    }
}
