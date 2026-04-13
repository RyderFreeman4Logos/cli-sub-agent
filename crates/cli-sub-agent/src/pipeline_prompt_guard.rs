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

/// Documented ceiling for fractal recursion (see `CSA_DEPTH` in architecture
/// docs). `load_and_validate` enforces this — the prompt-level guard only
/// warns the tool when a further `csa` invocation would exceed it.
const MAX_RECURSION_DEPTH: u32 = 5;

/// Build a depth-ceiling warning for tools dispatched by CSA.
///
/// Fractal recursion is a documented contract (Layer 1 → Layer 2 and beyond,
/// up to `MAX_RECURSION_DEPTH`). This guard is advisory only and fires just
/// before the ceiling so the tool can choose between (a) delegating once more
/// while depth still permits, or (b) doing the work inline. Returns `None`
/// below the near-ceiling threshold so legitimate sub-agent dispatch is not
/// discouraged — `load_and_validate` at `pipeline.rs` remains the hard
/// enforcement point.
pub(crate) fn anti_recursion_guard() -> Option<String> {
    let depth = current_depth();
    if depth + 1 < MAX_RECURSION_DEPTH {
        return None;
    }
    let remaining = MAX_RECURSION_DEPTH.saturating_sub(depth);
    Some(format!(
        "<csa-depth-ceiling depth=\"{depth}\" max=\"{MAX_RECURSION_DEPTH}\" remaining=\"{remaining}\">\n\
         NOTE: You are running at CSA recursion depth {depth} of {MAX_RECURSION_DEPTH}.\n\
         Further `csa run` / `csa review` / `csa debate` invocations count against the ceiling: \
         a sub-agent call from here would execute at depth {} and at most {remaining} \
         more levels are available before `load_and_validate` rejects the dispatch.\n\
         Prefer performing the remaining work directly unless delegation clearly \
         halves the work (e.g., a one-shot `csa review` whose sub-agents themselves \
         will not recurse further).\n\
         </csa-depth-ceiling>",
        depth.saturating_add(1),
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

/// SA mode caller guard block emitted to stdout.
///
/// When `--sa-mode true` is active at root depth (CSA_DEPTH=0), this block is
/// printed to stdout so the calling agent (e.g., Claude Code) sees it as part of
/// the Bash tool output. The structured XML tags reinforce that the caller must
/// operate as a pure orchestrator (Layer 0 Manager) and MUST NOT perform any
/// code-level work directly.
///
/// This guard fires at two points:
/// 1. At CSA startup — before session work begins (pre-session constraint).
/// 2. After session completes — reminder before caller takes next action.
pub(crate) const SA_MODE_CALLER_GUARD: &str = "\
<csa-caller-sa-guard>
SA MODE ACTIVE — You are Layer 0 Manager (pure orchestrator).

FORBIDDEN (SA contract violation — do NOT perform these actions):
• Read/edit/write source code files (*.rs, *.ts, *.py, etc.)
• Run build/test/lint/format commands (cargo, just, npm, etc.)
• Grep/Glob source code for investigation
• Inspect diffs or code content (git diff, git show, etc.)
• Read CSA transcripts or artifact contents directly

ALLOWED:
• Dispatch work via `csa run --sa-mode true`
• Read result.toml (structured report from CSA session)
• TaskCreate/TaskUpdate for tracking
• AskUserQuestion for user decisions
• Summarize result.toml conclusions to user

ALL implementation work MUST be delegated to CSA sub-agents.
Decisions MUST be based on result.toml reports, not direct code inspection.
</csa-caller-sa-guard>";

/// Emit SA mode caller guard to stdout.
///
/// Returns `true` if the guard was emitted. The guard is only emitted when
/// ALL conditions are met:
/// - `sa_mode` is `true`
/// - `depth` is 0 (root caller)
/// - `text_mode` is `true` (non-JSON output; avoids corrupting structured output)
pub(crate) fn emit_sa_mode_caller_guard(sa_mode: bool, depth: u32, text_mode: bool) -> bool {
    if !sa_mode || depth > 0 || !text_mode {
        return false;
    }
    println!("{SA_MODE_CALLER_GUARD}");
    true
}
