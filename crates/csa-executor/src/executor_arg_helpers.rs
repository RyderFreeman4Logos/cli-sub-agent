use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const GEMINI_INCLUDE_DIRS_ENV_KEYS: &[&str] = &[
    "CSA_GEMINI_INCLUDE_DIRECTORIES",
    "GEMINI_INCLUDE_DIRECTORIES",
];

/// `"default"` means "delegate model routing to gemini-cli", so omit `-m`.
pub(crate) fn effective_gemini_model_override(model_override: &Option<String>) -> Option<&str> {
    model_override
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.eq_ignore_ascii_case("default"))
        .filter(|model| !model.is_empty())
}

pub(crate) fn codex_notify_suppression_args(env: &HashMap<String, String>) -> Vec<String> {
    match env.get("CSA_SUPPRESS_NOTIFY").map(String::as_str) {
        Some("1") => vec!["-c".to_string(), "notify=[]".to_string()],
        _ => Vec::new(),
    }
}

pub(crate) fn gemini_include_directories(
    extra_env: Option<&HashMap<String, String>>,
    prompt: &str,
    execution_dir: Option<&Path>,
) -> Vec<String> {
    let mut directories = Vec::new();

    if let Some(dir) = execution_dir {
        push_unique_directory_string(&mut directories, dir.to_string_lossy().as_ref());
    }

    if let Some(env) = extra_env {
        let raw = GEMINI_INCLUDE_DIRS_ENV_KEYS
            .iter()
            .find_map(|key| env.get(*key))
            .map(String::as_str)
            .unwrap_or_default();

        for entry in raw.split([',', '\n']) {
            let directory = entry.trim();
            if directory.is_empty() {
                continue;
            }
            if Path::new(directory).is_relative() {
                if let Some(base) = execution_dir {
                    let combined = base.join(directory);
                    push_unique_directory_string(
                        &mut directories,
                        combined.to_string_lossy().as_ref(),
                    );
                } else {
                    push_unique_directory_string(&mut directories, directory);
                }
                continue;
            }
            push_unique_directory_string(&mut directories, directory);
        }
    }

    for directory in gemini_prompt_directories(prompt) {
        push_unique_directory_string(&mut directories, &directory);
    }

    directories
}

pub(crate) fn append_gemini_include_directories_args(cmd: &mut Command, directories: &[String]) {
    for directory in directories {
        cmd.arg("--include-directories").arg(directory);
    }
}

fn gemini_prompt_directories(prompt: &str) -> Vec<String> {
    let mut directories = Vec::new();
    let tokens: Vec<String> = prompt
        .split_whitespace()
        .map(trim_prompt_path_token)
        .filter(|token| !token.is_empty())
        .collect();

    let mut index = 0;
    while index < tokens.len() {
        if !tokens[index].starts_with('/') || tokens[index].contains("://") {
            index += 1;
            continue;
        }

        let mut candidate = String::new();
        let mut best_match: Option<(usize, PathBuf)> = None;
        for (end, token) in tokens.iter().enumerate().skip(index) {
            if end > index {
                if token.starts_with('/') {
                    break;
                }
                candidate.push(' ');
            }
            candidate.push_str(token);

            let path = Path::new(&candidate);
            if path.is_absolute() && path.exists() {
                best_match = Some((end, path.to_path_buf()));
            }
        }

        if let Some((end, path)) = best_match {
            let dir = if path.is_dir() {
                path
            } else if let Some(parent) = path.parent() {
                parent.to_path_buf()
            } else {
                index += 1;
                continue;
            };

            let normalized = fs::canonicalize(&dir).unwrap_or(dir);
            push_unique_directory_string(&mut directories, normalized.to_string_lossy().as_ref());
            index = end + 1;
        } else {
            index += 1;
        }
    }
    directories
}

fn trim_prompt_path_token(raw: &str) -> String {
    raw.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '`'
                | ','
                | ';'
                | ':'
                | '.'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
        )
    })
    .to_string()
}

fn push_unique_directory_string(directories: &mut Vec<String>, directory: &str) {
    let trimmed = directory.trim();
    if trimmed.is_empty() {
        return;
    }

    let path = Path::new(trimmed);
    // Never inject filesystem root as an include directory. In practice this
    // can explode workspace scan scope and trigger avoidable memory pressure.
    if is_filesystem_root(path) {
        return;
    }
    if directories.iter().any(|existing| existing == trimmed) {
        return;
    }
    directories.push(trimmed.to_string());
}

fn is_filesystem_root(path: &Path) -> bool {
    path.is_absolute() && path.parent().is_none()
}
