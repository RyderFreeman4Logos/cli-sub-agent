//! Project key resolution for memory-scoped session identification.
//!
//! Derives a stable, filesystem-safe project key from git remote URL,
//! git toplevel, or canonical path. Used by memory injection and session
//! isolation to distinguish projects sharing the same machine.

use std::path::Path;
use std::process::Command;

fn slugify_identifier(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;

    for ch in input.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if mapped == '-' {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        } else {
            out.push(mapped);
            last_dash = false;
        }
    }

    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn project_key_from_path(path: &Path) -> Option<String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    slugify_identifier(&canonical.to_string_lossy())
}

fn project_key_from_git_remote(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("config")
        .arg("--get")
        .arg("remote.origin.url")
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if remote.is_empty() {
        return None;
    }

    let sanitized = strip_url_credentials(&remote);
    slugify_identifier(&sanitized)
}

/// Strip userinfo (credentials) from a URL before using it as a key.
///
/// Handles both HTTPS (`https://token@host/org/repo.git`) and SSH
/// (`git@host:org/repo.git`) remote formats.
fn strip_url_credentials(url: &str) -> String {
    // HTTPS with embedded credentials: https://user:pass@host/path
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(at_pos) = after_scheme.find('@') {
            // Only strip if '@' comes before the first '/' (i.e. it's in the authority)
            let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
            if at_pos < slash_pos {
                return format!("{}{}", &url[..scheme_end + 3], &after_scheme[at_pos + 1..]);
            }
        }
        return url.to_string();
    }

    // SCP-style: user@host:org/repo.git — strip the userinfo prefix
    if let Some(at_pos) = url.find('@') {
        // Ensure '@' comes before ':' (SCP format, not a bare path)
        let colon_pos = url.find(':').unwrap_or(url.len());
        if at_pos < colon_pos {
            return url[at_pos + 1..].to_string();
        }
    }

    url.to_string()
}

fn project_key_from_git_toplevel(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if toplevel.is_empty() {
        return None;
    }

    project_key_from_path(Path::new(&toplevel))
}

pub(crate) fn resolve_memory_project_key(project_root: &Path) -> Option<String> {
    // Prefer explicit project_root argument (e.g. --cd) over inherited env var,
    // so nested sessions with --cd target the correct repo.
    project_key_from_git_remote(project_root)
        .or_else(|| project_key_from_git_toplevel(project_root))
        .or_else(|| project_key_from_path(project_root))
        .or_else(|| {
            std::env::var("CSA_PROJECT_ROOT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .and_then(|value| project_key_from_path(Path::new(&value)))
        })
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| project_key_from_path(&path))
        })
}
