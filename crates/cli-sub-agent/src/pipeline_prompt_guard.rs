pub(crate) const PROMPT_GUARD_CALLER_INJECTION_ENV: &str = "CSA_EMIT_CALLER_GUARD_INJECTION";
pub(crate) const COMPACT_SA_GUARD_ENV: &str = "CSA_SAY_COMPACT";
const SA_GUARD_TIER_ENV: &str = "CSA_SA_GUARD_TIER";
const SA_GUARD_TOOL_ENV: &str = "CSA_SA_GUARD_TOOL";

pub(super) fn should_emit_prompt_guard_to_caller(current_depth: u32) -> bool {
    // Prompt-guard reverse injection is only for the top-level caller.
    if current_depth > 0 {
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

/// Default ceiling for fractal recursion when no project config is in scope.
///
/// Matches `csa_config::ProjectMeta::max_recursion_depth` default; the real
/// hard-enforcement still lives in `pipeline::load_and_validate`, which reads
/// the project-configured ceiling.
pub(crate) const DEFAULT_MAX_RECURSION_DEPTH: u32 = 5;

/// Resolve the effective recursion ceiling from config (or fall back to the
/// documented default). Kept `pub(crate)` so tests and callers share the same
/// resolution logic — the ceiling is always aligned with the value that
/// `pipeline::load_and_validate` enforces at runtime.
pub(crate) fn effective_max_recursion_depth(config: Option<&csa_config::ProjectConfig>) -> u32 {
    config
        .map(|cfg| cfg.project.max_recursion_depth)
        .unwrap_or(DEFAULT_MAX_RECURSION_DEPTH)
}

/// Build a depth-ceiling warning for tools dispatched by CSA.
///
/// Fractal recursion is a documented contract (Layer 1 → Layer 2 and beyond,
/// up to the configured `max_recursion_depth` — default 5). This guard is
/// advisory only and fires just before the ceiling so the tool can choose
/// between (a) delegating once more while depth still permits, or (b) doing
/// the work inline. Returns `None` below the near-ceiling threshold so
/// legitimate sub-agent dispatch is not discouraged — `load_and_validate` at
/// `pipeline.rs` remains the hard enforcement point.
///
/// `config` is the project config for the caller; when `None`, the default
/// ceiling (`DEFAULT_MAX_RECURSION_DEPTH`) is used so the prompt-level guard
/// stays aligned with the same fallback that `pipeline::load_and_validate`
/// applies at runtime.
pub(crate) fn anti_recursion_guard(
    config: Option<&csa_config::ProjectConfig>,
    current_depth: u32,
) -> Option<String> {
    let depth = current_depth;
    let max_depth = effective_max_recursion_depth(config);
    if depth + 1 < max_depth {
        return None;
    }
    let remaining = max_depth.saturating_sub(depth);
    Some(format!(
        "<csa-depth-ceiling depth=\"{depth}\" max=\"{max_depth}\" remaining=\"{remaining}\">\n\
         NOTE: You are running at CSA recursion depth {depth} of {max_depth}.\n\
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

pub(super) fn emit_prompt_guard_to_caller(
    guard_block: &str,
    guard_count: usize,
    current_depth: u32,
) {
    if !should_emit_prompt_guard_to_caller(current_depth) || guard_block.trim().is_empty() {
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

NARROW EXCEPTION (orchestration loops only — pr-bot, dev2merge, similar):
• <=5-line mechanical fixes prescribed verbatim by cloud-bot/reviewer findings
  ('add this line', 'tighten this match', 'rename for consistency') MAY be
  applied directly when CSA cold-start cost would dwarf the actual change.
  Includes bot-prescribed test cleanup additions, mechanical case-sensitivity
  tightening, obvious typo fixes.
• Direct Edit is STILL FORBIDDEN for: (a) >5 lines, (b) cross-file changes,
  (c) any design judgment (which approach? which abstraction?),
  (d) substantive feature implementation, (e) when the user explicitly
  invoked /sa or csa run --sa-mode true as the top-level dispatch
  (they're paying for the audit).

ALLOWED:
• Dispatch work via `csa run --sa-mode true`
• Read result.toml (structured report from CSA session)
• TaskCreate/TaskUpdate for tracking
• AskUserQuestion for user decisions
• Summarize result.toml conclusions to user

ALL implementation work MUST be delegated to CSA sub-agents.
Decisions MUST be based on result.toml reports, not direct code inspection.
</csa-caller-sa-guard>";

pub(crate) fn set_sa_mode_caller_guard_context(tier: Option<&str>, tool: Option<&str>) {
    set_optional_env(SA_GUARD_TIER_ENV, tier);
    set_optional_env(SA_GUARD_TOOL_ENV, tool);
}

fn set_optional_env(key: &str, value: Option<&str>) {
    // SAFETY: process-level env is updated during CLI startup before async work begins.
    unsafe {
        match value {
            Some(value) if !value.trim().is_empty() => std::env::set_var(key, value),
            _ => std::env::remove_var(key),
        }
    }
}

fn env_flag_enabled(key: &str) -> bool {
    match std::env::var(key) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !normalized.is_empty() && !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        }
        Err(_) => false,
    }
}

pub(crate) fn compact_sa_guard_enabled() -> bool {
    env_flag_enabled(COMPACT_SA_GUARD_ENV)
}

pub(crate) fn format_compact_sa_mode_caller_guard(
    tier: Option<&str>,
    tool: Option<&str>,
) -> String {
    let mut line = String::from("<csa-caller-sa-guard:compact");
    push_compact_attr(&mut line, "tier", tier);
    push_compact_attr(&mut line, "tool", tool);
    line.push_str(" sa-mode=true contract=delegate-only-read-result-toml-no-direct-code/>");
    line
}

fn compact_sa_mode_caller_guard_from_env() -> String {
    let tier = std::env::var(SA_GUARD_TIER_ENV).ok();
    let tool = std::env::var(SA_GUARD_TOOL_ENV).ok();
    format_compact_sa_mode_caller_guard(tier.as_deref(), tool.as_deref())
}

fn push_compact_attr(line: &mut String, key: &str, value: Option<&str>) {
    let Some(value) = value.and_then(sanitize_compact_attr_value) else {
        return;
    };
    line.push(' ');
    line.push_str(key);
    line.push('=');
    line.push_str(&value);
}

fn sanitize_compact_attr_value(value: &str) -> Option<String> {
    let sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

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
    if compact_sa_guard_enabled() {
        println!("{}", compact_sa_mode_caller_guard_from_env());
    } else {
        println!("{SA_MODE_CALLER_GUARD}");
    }
    true
}
