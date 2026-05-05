use std::collections::HashMap;

pub(crate) const CSA_READONLY_SESSION_ENV: &str = "CSA_READONLY_SESSION";

pub(crate) fn with_readonly_session_env(
    base: Option<&HashMap<String, String>>,
    readonly: bool,
) -> Option<HashMap<String, String>> {
    let mut env = base.cloned().unwrap_or_default();
    if readonly {
        env.insert(CSA_READONLY_SESSION_ENV.to_string(), "1".to_string());
    }
    (!env.is_empty()).then_some(env)
}

/// Debate-only safety preamble injected into debate subprocess prompts.
///
/// Same shape as `review_cmd::ANTI_RECURSION_PREAMBLE`: the spawned tool is
/// constrained to read-only operations on the repository. Recursion-depth
/// enforcement is handled by `pipeline::prompt_guard` (warn near ceiling) and
/// `pipeline::load_and_validate` (hard reject above `MAX_RECURSION_DEPTH`), so
/// blanket "never call csa" text here would break the documented fractal
/// recursion contract (Layer 1 -> Layer 2 is legitimate).
pub(crate) const ANTI_RECURSION_PREAMBLE: &str = "\
CONTEXT: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Perform the debate task DIRECTLY using your own capabilities \
(Read, Grep, Glob, Bash for read-only git commands). \
IMPORTANT: This is a READ-ONLY analysis session. Do NOT modify, create, or delete any files. Report findings as text output only. \
DEBATE SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess.\n\n";
