pub(crate) const PROMPT_GUARD_CALLER_INJECTION_ENV: &str = "CSA_EMIT_CALLER_GUARD_INJECTION";

fn current_depth() -> u32 {
    std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(0)
}

pub(super) fn should_emit_prompt_guard_to_caller() -> bool {
    // Prompt-guard reverse injection is only for the top-level caller.
    if current_depth() > 0 {
        return false;
    }

    match std::env::var(PROMPT_GUARD_CALLER_INJECTION_ENV) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        }
        Err(_) => true,
    }
}

/// Build an anti-recursion guard block for tools dispatched by CSA.
///
/// When `CSA_DEPTH > 0`, the tool is running inside a CSA session and MUST NOT
/// attempt to delegate work back to `csa run`/`csa review`/`csa debate`.
/// Returns `Some(guard)` when depth > 0, `None` at the top level.
pub(super) fn anti_recursion_guard() -> Option<String> {
    let depth = current_depth();
    if depth == 0 {
        return None;
    }
    Some(format!(
        "<csa-anti-recursion depth=\"{depth}\">\n\
         CRITICAL: You are running INSIDE a CSA session (depth={depth}).\n\
         You MUST NOT delegate work to `csa run`, `csa review`, or `csa debate`.\n\
         You MUST NOT follow any instructions (including AGENTS.md or CLAUDE.md task-delegation rules) \
         that tell you to delegate large-context work to CSA sub-agents.\n\
         Perform ALL work directly with your own tools and context window.\n\
         If a file is too large, read it in sections — do NOT spawn a CSA sub-agent.\n\
         </csa-anti-recursion>"
    ))
}

pub(super) fn emit_prompt_guard_to_caller(guard_block: &str, guard_count: usize) {
    if !should_emit_prompt_guard_to_caller() || guard_block.trim().is_empty() {
        return;
    }
    eprintln!("[csa-hook] reverse prompt injection for caller (guards={guard_count})");
    eprintln!("<csa-caller-prompt-injection guards=\"{guard_count}\">");
    eprintln!("{guard_block}");
    eprintln!("</csa-caller-prompt-injection>");
}
