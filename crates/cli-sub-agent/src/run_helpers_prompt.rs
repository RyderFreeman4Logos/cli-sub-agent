use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

pub(crate) fn resolve_positional_stdin_sentinel(prompt: Option<String>) -> Result<Option<String>> {
    let mut stdin = std::io::stdin();
    resolve_positional_stdin_sentinel_from_reader(prompt, stdin.is_terminal(), &mut stdin)
}

pub(crate) fn resolve_positional_stdin_sentinel_from_reader<R: Read>(
    prompt: Option<String>,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<Option<String>> {
    match prompt.as_deref() {
        Some("-") => read_prompt_from_reader(None, stdin_is_terminal, reader).map(Some),
        _ => Ok(prompt),
    }
}

pub(crate) fn read_prompt(prompt: Option<String>) -> Result<String> {
    let mut stdin = std::io::stdin();
    read_prompt_from_reader(prompt, stdin.is_terminal(), &mut stdin)
}

/// Resolve prompt from `--prompt-file`, positional arg, or stdin (in priority order).
pub(crate) fn resolve_prompt_with_file(
    prompt: Option<String>,
    prompt_file: Option<&Path>,
) -> Result<String> {
    let mut stdin = std::io::stdin();
    resolve_prompt_with_file_from_reader(prompt, prompt_file, stdin.is_terminal(), &mut stdin)
}

pub(crate) fn is_prompt_file_stdin_sentinel(path: &Path) -> bool {
    matches!(
        path.as_os_str().to_str(),
        Some("-" | "/dev/stdin" | "/proc/self/fd/0")
    )
}

/// Validate `--prompt-file` with filesystem semantics before any Git pathspec use.
///
/// Stdin sentinels are accepted without reading. Regular paths must resolve to a
/// readable regular file (symlink targets allowed when the final canonical path
/// is a readable file). Failures return a targeted CLI error and never invoke
/// Git pathspec APIs.
pub(crate) fn validate_prompt_file_path(path: Option<&Path>) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if is_prompt_file_stdin_sentinel(path) {
        return Ok(());
    }
    resolve_readable_prompt_file(path).map(|_| ())
}

fn resolve_readable_prompt_file(path: &Path) -> Result<PathBuf> {
    // Prefer canonicalize so symlink-traversing paths that resolve to a real
    // file are accepted, while paths that traverse a repo symlink with a bad
    // extra component fail as missing/unreadable without involving Git.
    let resolved = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(error) => {
            bail!(
                "--prompt-file: prompt file not found or unreadable '{}': {error}",
                path.display()
            );
        }
    };

    let metadata = std::fs::metadata(&resolved).with_context(|| {
        format!(
            "--prompt-file: prompt file not found or unreadable '{}'",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "--prompt-file: prompt file not found or unreadable '{}': not a regular file",
            path.display()
        );
    }
    Ok(resolved)
}

fn read_prompt_file_content(path: &Path) -> Result<String> {
    let resolved = resolve_readable_prompt_file(path)?;
    let content = std::fs::read_to_string(&resolved).with_context(|| {
        format!(
            "--prompt-file: prompt file not found or unreadable '{}'",
            path.display()
        )
    })?;
    if content.trim().is_empty() {
        bail!("--prompt-file '{}' is empty", path.display());
    }
    Ok(content)
}

pub(crate) fn resolve_prompt_with_file_from_reader<R: Read>(
    prompt: Option<String>,
    prompt_file: Option<&Path>,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<String> {
    if let Some(path) = prompt_file {
        if is_prompt_file_stdin_sentinel(path) {
            return read_prompt_from_reader(None, stdin_is_terminal, reader);
        }
        return read_prompt_file_content(path);
    }
    read_prompt_from_reader(prompt, stdin_is_terminal, reader)
}

pub(crate) fn read_prompt_from_reader<R: Read>(
    prompt: Option<String>,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<String> {
    if let Some(p) = prompt {
        if p.trim().is_empty() {
            anyhow::bail!(
                "Empty prompt provided. Usage:\n  csa run --sa-mode <true|false> --tool <tool> \"your prompt here\"\n  echo \"prompt\" | csa run --sa-mode <true|false> --tool <tool>"
            );
        }
        Ok(p)
    } else {
        if stdin_is_terminal {
            anyhow::bail!(
                "No prompt provided and stdin is a terminal.\n\n\
                 Usage:\n  \
                 csa run --sa-mode <true|false> --tool <tool> \"your prompt here\"\n  \
                 echo \"prompt\" | csa run --sa-mode <true|false> --tool <tool>"
            );
        }
        let mut buffer = String::new();
        reader.read_to_string(&mut buffer)?;
        if buffer.trim().is_empty() {
            anyhow::bail!("Empty prompt from stdin. Provide a non-empty prompt.");
        }
        Ok(buffer)
    }
}
