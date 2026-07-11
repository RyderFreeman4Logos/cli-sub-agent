use super::*;

pub(crate) struct SkillResolution {
    pub(crate) prompt_text: String,
    pub(crate) frontmatter_difficulty: Option<String>,
    pub(crate) resolved_skill: Option<ResolvedSkill>,
    pub(crate) tool: Option<csa_core::types::ToolArg>,
    pub(crate) model: Option<String>,
    pub(crate) thinking: Option<String>,
}

pub(crate) struct SkillPromptSource<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) skill_source_dir: &'a Path,
    pub(crate) extra_context_dir: &'a Path,
    pub(crate) skill_md: &'a str,
    pub(crate) agent_config: Option<&'a AgentConfig>,
}

pub(crate) fn build_skill_prompt_parts(source: SkillPromptSource<'_>) -> Vec<String> {
    let mut parts = vec![
        "<skill-mode>executor</skill-mode>".to_string(),
        format!(
            "<workspace-scope root=\"{}\">\nSTRICT SCOPE: Only read/write files under this root. If a tool returns workspace-boundary errors (for example, 'Path not in workspace'), stop and report failure instead of retrying sibling paths.\n</workspace-scope>",
            source.project_root.display()
        ),
        format!(
            "<skill-source path=\"{}\">\nResolve relative skill references from this directory.\n</skill-source>",
            source.skill_source_dir.display()
        ),
        crate::skill_repo::sanitize_skill_md(source.skill_md),
    ];

    if let Some(agent) = source.agent_config {
        for extra in &agent.extra_context {
            let extra_path = source.extra_context_dir.join(extra);
            match std::fs::read_to_string(&extra_path) {
                Ok(content) => {
                    parts.push(format!(
                        "<context-file path=\"{extra}\">\n{content}\n</context-file>"
                    ));
                }
                Err(e) => {
                    warn!(path = %extra, error = %e, "Failed to load skill extra_context file");
                }
            }
        }
    }

    parts
}

/// Resolve the skill (if any), build the prompt, and apply agent config
/// overrides for tool/model/thinking.
pub(crate) fn resolve_skill_and_prompt(
    skill: Option<&str>,
    prompt: Option<String>,
    tool: Option<csa_core::types::ToolArg>,
    model: Option<String>,
    thinking: Option<String>,
    project_root: &Path,
) -> Result<SkillResolution> {
    let resolved_skill = if let Some(skill_name) = skill {
        Some(skill_resolver::resolve_skill(skill_name, project_root)?)
    } else {
        None
    };

    let (prompt_text, frontmatter_difficulty) = if let Some(ref sk) = resolved_skill {
        // Skills execute inside `csa run` as the leaf executor. Inject an
        // explicit mode marker so skill docs can branch deterministically and
        // avoid orchestrator-style recursive `csa run` loops.
        let mut parts = build_skill_prompt_parts(SkillPromptSource {
            project_root,
            skill_source_dir: &sk.dir,
            extra_context_dir: &sk.dir,
            skill_md: &sk.skill_md,
            agent_config: sk.agent_config(),
        });

        let mut difficulty = None;
        if let Some(user_prompt) = prompt {
            let parsed = crate::difficulty_routing::strip_difficulty_frontmatter(user_prompt)?;
            difficulty = parsed.difficulty;
            parts.push(format!("---\n\n{}", parsed.prompt));
        }

        (parts.join("\n\n"), difficulty)
    } else {
        let parsed = crate::difficulty_routing::strip_difficulty_frontmatter(read_prompt(prompt)?)?;
        (parsed.prompt, parsed.difficulty)
    };

    // Apply skill agent config overrides for tool/model when CLI didn't specify.
    let skill_agent = resolved_skill.as_ref().and_then(|sk| sk.agent_config());
    let tool = if tool.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| parse_tool_name(&t.tool).ok())
            .map(csa_core::types::ToolArg::Specific)
            .or(tool)
    } else {
        tool
    };
    let model = if model.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.model.clone())
            .or(model)
    } else {
        model
    };
    let thinking = if thinking.is_none() {
        skill_agent
            .and_then(|a| a.tools.first())
            .and_then(|t| t.thinking_budget.clone())
            .or(thinking)
    } else {
        thinking
    };

    Ok(SkillResolution {
        prompt_text,
        frontmatter_difficulty,
        resolved_skill,
        tool,
        model,
        thinking,
    })
}

/// Resolve the `--return-to` target to a concrete session ID.
pub(crate) fn resolve_return_target_session_id(
    return_target: &ReturnTarget,
    project_root: &Path,
    fork_source_ref: Option<&str>,
    parent_flag: Option<&str>,
    startup_session_id: Option<&str>,
) -> Result<Option<String>> {
    match return_target {
        ReturnTarget::Last => {
            let sessions = csa_session::list_sessions(project_root, None)?;
            let (selected_id, _) = resolve_last_session_selection(sessions)?;
            Ok(Some(selected_id))
        }
        ReturnTarget::SessionId(session_ref) => {
            let resolved = resolve_session_reference(project_root, session_ref)?;
            Ok(Some(resolved))
        }
        ReturnTarget::Auto => {
            let candidate = fork_source_ref
                .map(ToOwned::to_owned)
                .or_else(|| parent_flag.map(ToOwned::to_owned))
                .or_else(|| startup_session_id.map(ToOwned::to_owned));

            if let Some(session_ref) = candidate {
                let resolved = resolve_session_reference(project_root, &session_ref)?;
                Ok(Some(resolved))
            } else {
                Ok(None)
            }
        }
    }
}
