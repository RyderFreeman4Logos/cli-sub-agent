pub(super) const PROMPT_GUARD_CALLER_INJECTION_ENV: &str = "CSA_EMIT_CALLER_GUARD_INJECTION";

pub(super) fn should_emit_prompt_guard_to_caller() -> bool {
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
