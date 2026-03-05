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

pub(super) fn emit_prompt_guard_to_caller(guard_block: &str, guard_count: usize) {
    if !should_emit_prompt_guard_to_caller() || guard_block.trim().is_empty() {
        return;
    }
    eprintln!("[csa-hook] reverse prompt injection for caller (guards={guard_count})");
    eprintln!("<csa-caller-prompt-injection guards=\"{guard_count}\">");
    eprintln!("{guard_block}");
    eprintln!("</csa-caller-prompt-injection>");
}
