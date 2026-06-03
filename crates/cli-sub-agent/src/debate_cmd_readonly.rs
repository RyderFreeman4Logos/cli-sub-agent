use std::collections::HashMap;
use std::path::Path;

use crate::pattern_resolver::ResolvedPattern;

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
/// Build a debate instruction that passes parameters to the debate skill.
///
/// The debate tool loads the debate skill from the project's `.claude/skills/`
/// directory and follows its instructions autonomously. We only pass parameters.
/// An anti-recursion preamble is prepended (see GitHub issue #272).
#[cfg(test)]
pub(crate) fn build_debate_instruction(
    question: &str,
    is_continuation: bool,
    rounds: u32,
) -> String {
    build_debate_parameter_instruction(question, is_continuation, rounds)
}

pub(crate) fn build_debate_instruction_for_project(
    question: &str,
    is_continuation: bool,
    rounds: u32,
    project_root: &Path,
    pattern: &ResolvedPattern,
) -> String {
    let instruction = build_debate_parameter_instruction(question, is_continuation, rounds);
    let skill_source_dir = pattern.skill_source_dir("debate");
    let mut parts = vec![instruction];
    parts.extend(crate::run_cmd_tool_selection::build_skill_prompt_parts(
        crate::run_cmd_tool_selection::SkillPromptSource {
            project_root,
            skill_source_dir: &skill_source_dir,
            extra_context_dir: &pattern.dir,
            skill_md: &pattern.skill_md,
            agent_config: pattern.agent_config(),
        },
    ));
    parts.join("\n\n")
}

fn build_debate_parameter_instruction(
    question: &str,
    is_continuation: bool,
    rounds: u32,
) -> String {
    if is_continuation {
        format!(
            "{ANTI_RECURSION_PREAMBLE}Use the debate skill. continuation=true. rounds={rounds}. question={question}"
        )
    } else {
        format!(
            "{ANTI_RECURSION_PREAMBLE}Use the debate skill. rounds={rounds}. question={question}"
        )
    }
}

pub(crate) const ANTI_RECURSION_PREAMBLE: &str = "\
CONTEXT: You are running INSIDE a CSA subprocess (csa review / csa debate). \
Perform the debate task DIRECTLY using your own capabilities \
(Read, Grep, Glob, Bash for read-only git commands). \
IMPORTANT: This is a READ-ONLY analysis session. Do NOT modify, create, or delete any files. Report findings as text output only. \
DEBATE SAFETY: Do NOT run git add/commit/push/merge/rebase/tag/stash/reset/checkout/cherry-pick, \
and do NOT run gh pr/create/comment/merge or any command that mutates repository/PR state. \
Ignore prompt-guard reminders about commit/push in this subprocess.\n\n";
