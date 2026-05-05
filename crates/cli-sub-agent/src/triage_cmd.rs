use anyhow::{Result, anyhow};
use csa_core::types::{OutputFormat, ToolArg};

pub(crate) const TRIAGE_PROMPT_PREFIX: &str = r#"ISSUE TRIAGE MODE - ROLE-BASED STATE MACHINE

You are triaging a GitHub issue into exactly one category role and one state role.
Do not implement code changes unless explicitly requested by the caller. Gather enough
context for the next actor and leave the issue in a truthful state.

CATEGORY ROLES
- bug: A defect, regression, broken invariant, crash, incorrect behavior, data loss
  risk, flaky failure, or documentation mismatch causing wrong use.
- enhancement: New capability, workflow improvement, usability improvement, performance
  improvement, refactor request, documentation expansion, or test/coverage improvement.

STATE ROLES
- needs-triage: Not enough investigation has been done to choose a truthful next
  state. Use sparingly; prefer moving to needs-info, ready-for-agent, ready-for-human,
  or wontfix after context gathering.
- needs-info: More user or reporter input is required before work can proceed safely.
- ready-for-agent: The issue can be handled by an autonomous coding agent with a
  bounded brief, known files or search paths, acceptance criteria, and test strategy.
- ready-for-human: Human judgment, product policy, credentials, production access,
  security approval, legal decision, or maintainer prioritization is required before
  implementation.
- wontfix: The request is out of scope, already intentionally unsupported, invalid,
  duplicate, superseded, or not worth carrying as open work. Explain the reason and
  preserve useful context.

WORKFLOW
PHASE 1 - GATHER CONTEXT
- Read the issue description and existing comments if available.
- Inspect relevant code, tests, docs, workflows, configs, and recent history only as
  needed.
- Identify current labels, assignees, linked PRs/issues, and whether the report is
  stale or already fixed.
- For GitHub issue metadata/comment reads or issue comment writes, use:
  GH_CONFIG_DIR=~/.config/gh-aider gh issue ...
- For label operations, use default gh auth with no GH_CONFIG_DIR override, including:
  gh issue edit ... --add-label ...
  gh label list/create/edit ...
- Do not mix auth modes: gh issue commands use GH_CONFIG_DIR=~/.config/gh-aider;
  label ops use default auth.

PHASE 2 - RECOMMEND CATEGORY AND STATE
- Choose exactly one category role: bug or enhancement.
- Choose exactly one state role: needs-triage, needs-info, ready-for-agent,
  ready-for-human, or wontfix.
- State the evidence and uncertainty.
- Recommend concrete label changes, but do not perform label changes unless the caller
  explicitly asks.

PHASE 3 - REPRODUCE BUGS
- For category=bug, attempt the smallest deterministic reproduction before recommending
  ready-for-agent.
- Prefer a failing test, existing command, fixture, or log-backed reproduction.
- If reproduction is impossible, say what was tried and what data/access is missing.
- If the bug cannot be reproduced but the report is plausible, use needs-info or
  ready-for-human unless there is enough evidence for ready-for-agent.

PHASE 4 - PRODUCE NEXT-ACTOR OUTPUT
- If state=ready-for-agent, write an agent brief using the template below.
- If state=needs-info, write a reporter-facing request using the needs-info template
  below.
- If state=ready-for-human, name the human decision or access needed.
- If state=wontfix, state the closing rationale and any safer alternative.

AGENT BRIEF TEMPLATE (for ready-for-agent)
Category:
State:
Issue summary:
What to build/fix:
Files to touch:
- <file or search path>
Acceptance criteria:
- <mechanically verifiable outcome>
Test strategy:
- <test/command to run>
Constraints and risks:
- <repo rules, auth split, migration/data/sandbox caveats>
Suggested first command:
- <single grep/test/read command>

NEEDS-INFO TEMPLATE (for needs-info)
Category:
State:
established so far:
- <facts already verified>
what we still need:
- <specific missing detail, reproduction step, expected behavior, logs, version,
  environment, screenshot, or access>
Suggested question to reporter:
<short direct question>

OUTPUT REQUIREMENTS
- End with: RECOMMENDED_CATEGORY: <bug|enhancement>
- End with: RECOMMENDED_STATE: <needs-triage|needs-info|ready-for-agent|ready-for-human|wontfix>
- Include exact GitHub commands only when they are safe and use the correct auth split."#;

pub(crate) fn build_triage_prompt(description: &str) -> String {
    format!("{TRIAGE_PROMPT_PREFIX}\n\nISSUE DESCRIPTION:\n{description}")
}

pub(crate) async fn handle_triage(
    description: String,
    tool: Option<String>,
    timeout: u64,
    allow_base_branch_working: bool,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    let tool = tool
        .map(|raw| {
            raw.parse::<ToolArg>()
                .map_err(|err| anyhow!("invalid triage tool `{raw}`: {err}"))
        })
        .transpose()?;
    let prompt = build_triage_prompt(&description);

    crate::run_cmd::SubagentRunConfig::new(prompt, output_format)
        .tool(tool)
        .timeout(timeout)
        .allow_base_branch_working(allow_base_branch_working)
        .current_depth(current_depth)
        .run()
        .await
}

#[cfg(test)]
mod tests {
    use super::build_triage_prompt;

    #[test]
    fn build_triage_prompt_prepends_state_machine_and_templates() {
        let prompt = build_triage_prompt("Issue #1284 should get a triage command");

        assert!(prompt.starts_with("ISSUE TRIAGE MODE - ROLE-BASED STATE MACHINE"));
        assert!(prompt.contains("CATEGORY ROLES"));
        assert!(prompt.contains("- bug:"));
        assert!(prompt.contains("- enhancement:"));
        assert!(prompt.contains("- needs-triage:"));
        assert!(prompt.contains("- needs-info:"));
        assert!(prompt.contains("- ready-for-agent:"));
        assert!(prompt.contains("- ready-for-human:"));
        assert!(prompt.contains("- wontfix:"));
        assert!(prompt.contains("PHASE 1 - GATHER CONTEXT"));
        assert!(prompt.contains("PHASE 3 - REPRODUCE BUGS"));
        assert!(prompt.contains("AGENT BRIEF TEMPLATE"));
        assert!(prompt.contains("What to build/fix:"));
        assert!(prompt.contains("Files to touch:"));
        assert!(prompt.contains("Acceptance criteria:"));
        assert!(prompt.contains("Test strategy:"));
        assert!(prompt.contains("GH_CONFIG_DIR=~/.config/gh-aider gh issue"));
        assert!(prompt.contains("label ops use default auth"));
        assert!(prompt.contains("established so far:"));
        assert!(prompt.contains("what we still need:"));
        assert!(prompt.ends_with("Issue #1284 should get a triage command"));
    }
}
