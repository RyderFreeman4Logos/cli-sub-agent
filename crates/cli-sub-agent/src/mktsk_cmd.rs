use anyhow::{Result, anyhow};
use csa_core::types::{OutputFormat, ToolArg};

pub(crate) const MKTSK_PROMPT_PREFIX: &str = r#"TASK DECOMPOSITION MODE - COMPACT-RESILIENT TODO BREAKDOWN

You are translating a CSA TODO plan into granular task entries that survive context
compaction and session boundaries. Do not implement production code unless the caller
explicitly asks for it. Your job is to read the current plan, decompose it into
self-contained tasks, and capture the execution context each future agent needs.

WORKFLOW
1. Read the plan with `csa todo show`.
2. Break the plan into individual task entries.
3. For each task, write a compact-resilient description that can stand alone after
   context compaction.
4. Set dependency chains between tasks so execution order is explicit.

PLAN READ REQUIREMENTS
- If the caller provided a plan timestamp, read it with:
  `csa todo show --timestamp <TIMESTAMP>`
- Otherwise read the latest plan with:
  `csa todo show`
- Do not guess plan contents without reading the plan first.

TASK ENTRY REQUIREMENTS
For each task entry, include all of the following:
- Title
- Related issue numbers
- Branch name
- Files to modify or search paths
- Fix/build approach
- Dependencies on other tasks
- DONE WHEN criteria with mechanically verifiable outcomes

OUTPUT FORMAT
- Start with `PLAN_SOURCE: <latest|timestamp>`
- Then list the tasks in execution order.
- Use one section per task with a stable identifier such as `TASK 1`, `TASK 2`, etc.
- Make each task self-contained: a future agent should not need the original plan to
  understand what to do.
- End with `DEPENDENCY_GRAPH:` followed by a concise dependency summary."#;

pub(crate) fn build_mktsk_prompt(description: &str, todo: Option<&str>) -> String {
    let plan_source = match todo {
        Some(timestamp) => format!(
            "PLAN TO READ:\nUse `csa todo show --timestamp {timestamp}` before decomposing tasks."
        ),
        None => {
            "PLAN TO READ:\nUse `csa todo show` to read the latest plan before decomposing tasks."
                .to_string()
        }
    };

    format!("{MKTSK_PROMPT_PREFIX}\n\n{plan_source}\n\nTASK DECOMPOSITION REQUEST:\n{description}")
}

pub(crate) async fn handle_mktsk(
    description: String,
    todo: Option<String>,
    tool: Option<String>,
    timeout: u64,
    allow_base_branch_working: bool,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    let tool = tool
        .map(|raw| {
            raw.parse::<ToolArg>()
                .map_err(|err| anyhow!("invalid mktsk tool `{raw}`: {err}"))
        })
        .transpose()?;
    let prompt = build_mktsk_prompt(&description, todo.as_deref());

    crate::run_cmd::SubagentRunConfig::new(prompt, output_format)
        .tool(tool)
        .timeout(timeout)
        .allow_base_branch_working(allow_base_branch_working)
        .current_depth(current_depth)
        .run()
        .await
}

pub(crate) async fn handle_mktsk_args(
    args: crate::cli::MktskArgs,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    handle_mktsk(
        args.description,
        args.todo,
        args.tool,
        args.timeout,
        args.allow_base_branch_working,
        current_depth,
        output_format,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{MKTSK_PROMPT_PREFIX, build_mktsk_prompt};

    #[test]
    fn build_mktsk_prompt_targets_latest_plan_by_default() {
        let prompt = build_mktsk_prompt("Decompose issue #1305 into tasks", None);

        assert!(prompt.starts_with("TASK DECOMPOSITION MODE - COMPACT-RESILIENT TODO BREAKDOWN"));
        assert!(MKTSK_PROMPT_PREFIX.contains("Read the plan with `csa todo show`."));
        assert!(MKTSK_PROMPT_PREFIX.contains("Related issue numbers"));
        assert!(MKTSK_PROMPT_PREFIX.contains("Branch name"));
        assert!(MKTSK_PROMPT_PREFIX.contains("Files to modify or search paths"));
        assert!(MKTSK_PROMPT_PREFIX.contains("Fix/build approach"));
        assert!(MKTSK_PROMPT_PREFIX.contains("DONE WHEN criteria"));
        assert!(MKTSK_PROMPT_PREFIX.contains("DEPENDENCY_GRAPH:"));
        assert!(prompt.contains("Use `csa todo show` to read the latest plan"));
        assert!(prompt.ends_with("Decompose issue #1305 into tasks"));
    }

    #[test]
    fn build_mktsk_prompt_uses_explicit_timestamp_when_provided() {
        let prompt =
            build_mktsk_prompt("Decompose issue #1305 into tasks", Some("20260505-123456"));

        assert!(
            prompt.contains(
                "Use `csa todo show --timestamp 20260505-123456` before decomposing tasks."
            )
        );
        assert!(prompt.contains("TASK DECOMPOSITION REQUEST:"));
    }
}
