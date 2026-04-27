use anyhow::{Context, Result};
use std::io::{IsTerminal, Read};
use std::path::Path;

pub(crate) fn resolve_positional_stdin_sentinel(prompt: Option<String>) -> Result<Option<String>> {
    let mut stdin = std::io::stdin();
    resolve_positional_stdin_sentinel_from_reader(prompt, stdin.is_terminal(), &mut stdin)
}

#[cfg(test)]
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

#[cfg(not(test))]
fn resolve_positional_stdin_sentinel_from_reader<R: Read>(
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
    if let Some(path) = prompt_file {
        if is_prompt_file_stdin_sentinel(path) {
            return read_prompt(None);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("--prompt-file: failed to read '{}'", path.display()))?;
        if content.trim().is_empty() {
            anyhow::bail!("--prompt-file '{}' is empty", path.display());
        }
        return Ok(content);
    }
    read_prompt(prompt)
}

pub(crate) fn is_prompt_file_stdin_sentinel(path: &Path) -> bool {
    matches!(
        path.as_os_str().to_str(),
        Some("-" | "/dev/stdin" | "/proc/self/fd/0")
    )
}

#[cfg(test)]
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

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("--prompt-file: failed to read '{}'", path.display()))?;
        if content.trim().is_empty() {
            anyhow::bail!("--prompt-file '{}' is empty", path.display());
        }
        return Ok(content);
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
