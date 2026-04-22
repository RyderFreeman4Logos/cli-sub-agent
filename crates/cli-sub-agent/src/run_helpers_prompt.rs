use anyhow::Result;
use std::io::{IsTerminal, Read};

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

fn read_prompt_from_reader<R: Read>(
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
