use super::*;

pub(crate) fn resolve_tool_and_model(
    request: RoutingRequest<'_>,
) -> Result<(ToolName, Option<String>, Option<String>)> {
    // Destructure request at the top for clean access to all fields
    let RoutingRequest {
        tool,
        model_spec,
        model,
        thinking,
        config,
        global_config,
        model_catalog,
        project_root,
        force,
        force_override_user_config,
        needs_edit,
        tier,
        force_ignore_tier_setting,
        tier_bypass_allowed,
        tool_is_auto_resolved,
    } = request;
    let shipped_catalog;
    let model_catalog = if let Some(catalog) = model_catalog {
        catalog
    } else {
        shipped_catalog = csa_config::EffectiveModelCatalog::shipped()?;
        &shipped_catalog
    };
    let runtime_env_for_tool = |tool_name: &str| {
        global_config.and_then(|cfg| {
            cfg.build_execution_env(tool_name, csa_config::ExecutionEnvOptions::default())
        })
    };
    let tiers_configured = config.is_some_and(|c| !c.tiers.is_empty());
    let bypass_tier = force_ignore_tier_setting || tier_bypass_allowed;
    let exact_selection_active = model_spec.is_some();

    // Enforce tier routing: block direct --tool/--model/--thinking when tiers are configured,
    // unless a shared tier-bypass gate has accepted an explicit bypass. --model-spec is
    // exact selection, but it must still match a configured tier model unless bypassed below.
    // Auto-resolved tools (from HeterogeneousPreferred etc.) don't count as user-explicit.
    let tool_triggers_enforcement = tool.is_some() && !tool_is_auto_resolved;
    validate_tool_tier_override_flags(tool_triggers_enforcement, tier, force_ignore_tier_setting)?;
    if tiers_configured
        && !bypass_tier
        && tier.is_none()
        && !exact_selection_active
        && (tool_triggers_enforcement || model.is_some() || thinking.is_some())
    {
        let cfg = config.unwrap();
        let mut tier_list = String::new();
        for name in cfg.tiers.keys() {
            if !tier_list.is_empty() {
                tier_list.push_str(", ");
            }
            tier_list.push_str(name);
        }
        let alias_hint = cfg.format_tier_aliases();
        anyhow::bail!(
            "Direct --tool/--model/--thinking is restricted when tiers are configured.\n\
             Use --tier <name> to specify which tier's model/thinking config to use, \
             or set [tier_policy].allow_force_bypass = true in the global CSA config \
             for emergency bypasses.\n\
             Available tiers: [{tier_list}]{alias_hint}\n\
             Hint: omit --tool entirely to use auto-selection, or use --tool auto"
        );
    }

    if force_ignore_tier_setting && tiers_configured && tier.is_none() && model_spec.is_none() {
        let configured_tool_defaults = tool.and_then(|tool_name| {
            config.map(|cfg| {
                (
                    cfg.tool_default_model(tool_name.as_str()).is_some(),
                    cfg.tool_default_thinking(tool_name.as_str()).is_some(),
                )
            })
        });
        let has_configured_default_model =
            configured_tool_defaults.is_some_and(|(has_model, _)| has_model);
        let has_configured_default_thinking =
            configured_tool_defaults.is_some_and(|(_, has_thinking)| has_thinking);

        let mut missing = Vec::new();
        if tool.is_none() {
            missing.push("--tool");
        }
        if model.is_none() && !has_configured_default_model {
            missing.push("--model");
        }
        if thinking.is_none() && !has_configured_default_thinking {
            missing.push("--thinking");
        }

        if !missing.is_empty() {
            anyhow::bail!(
                "When using --force-ignore-tier-setting to bypass tier enforcement, \
                 you must provide complete model specification.\n\
                 Missing required flags: {}\n\
                 Example: csa run --sa-mode <true|false> --force-ignore-tier-setting --tool claude-code \
                 --model claude-3-5-sonnet-20241022 --thinking medium \"prompt\"",
                missing.join(", ")
            );
        }
    }

    // Validate and canonicalize tier selector (accepts direct tier names and tier_mapping aliases).
    // Even in bypass_tier mode, resolve aliases so resolve_tool_from_tier gets a canonical name.
    let canonical_tier: Option<String> = if let Some(tier_name) = tier {
        if let Some(cfg) = config {
            if let Some(canonical) = cfg.resolve_tier_selector(tier_name) {
                Some(canonical)
            } else if bypass_tier {
                // bypass mode: tolerate unknown selector (pass through as-is)
                Some(tier_name.to_string())
            } else {
                let available: Vec<&str> = cfg.tiers.keys().map(|k| k.as_str()).collect();
                let alias_hint = cfg.format_tier_aliases();
                let suggest_hint = cfg
                    .suggest_tier(tier_name)
                    .map(|s| format!("\nDid you mean '{s}'?"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "Tier selector '{}' not found.\n\
                     Available tiers: [{}]{alias_hint}{suggest_hint}",
                    tier_name,
                    available.join(", ")
                );
            }
        } else {
            anyhow::bail!(
                "Tier '{}' specified but no project config found. \
                 Run 'csa init --full' to create a config with tier definitions.",
                tier_name
            );
        }
    } else {
        None
    };

    // Case 0: --tier provided -> resolve tool/model from tier definition.
    // A user-explicit `--tool` is a soft preference inside the selected tier:
    // prefer matching candidates first, then keep the rest of the tier failover chain.
    if let Some(ref canonical_name) = canonical_tier
        && let Some(cfg) = config
    {
        let resolution = if let Some(requested_tool) = tool.filter(|_| !tool_is_auto_resolved) {
            let preference_order = [requested_tool.as_str().to_string()];
            resolve_preferred_tool_from_tier_with_catalog(
                canonical_name,
                cfg,
                global_config,
                model_catalog,
                None,
                &preference_order,
                &[],
            )?
        } else if let Some(resolution) = resolve_tool_from_tier_with_catalog(
            canonical_name,
            cfg,
            global_config,
            model_catalog,
            None,
            &[],
            &[],
        )? {
            resolution
        } else {
            anyhow::bail!(
                "No available tool found in tier '{}'. Check that at least one tool \
                     in the tier is enabled and installed.",
                canonical_name
            );
        };

        // Flow resolved tool through existing enforcement checks.
        cfg.enforce_tool_enabled(resolution.tool.as_str(), force_override_user_config)?;
        if !force {
            cfg.enforce_tier_whitelist(resolution.tool.as_str(), Some(&resolution.model_spec))?;
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((resolution.tool, Some(resolution.model_spec), resolved_model));
    }

    // Case 1: model_spec provided → parse it to get tool. --model-spec is an
    // exact selection, but configured tiers still whitelist allowed specs.
    if let Some(spec) = model_spec {
        let parsed = ModelSpec::parse(spec)?;
        let known_tools: Vec<&'static str> = csa_config::global::all_known_tools()
            .iter()
            .map(|tool| tool.as_str())
            .collect();
        parsed
            .validate_with_catalog(model_catalog, &known_tools)
            .map_err(|error| anyhow::anyhow!("catalog admission rejected '{spec}': {error}"))?;
        let tool_name = parse_tool_name(&parsed.tool)?;
        if let Some(requested_tool) = tool.filter(|_| !tool_is_auto_resolved)
            && requested_tool != tool_name
        {
            return Err(routing_conflict_error(format!(
                "Conflicting routing flags: --tool {} does not match --model-spec {}.\n\
                 The model spec selects tool {}. Use a matching --tool value or omit --tool.",
                requested_tool.as_str(),
                spec,
                tool_name.as_str()
            )));
        }
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
            if !force && !bypass_tier {
                if cfg.tiers.is_empty() {
                    enforce_model_spec_matches_tool_default(cfg, &parsed, spec)?;
                } else {
                    cfg.enforce_tier_whitelist(tool_name.as_str(), Some(spec))?;
                }
            }
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        return Ok((tool_name, Some(spec.to_string()), resolved_model));
    }

    // Case 2: tool provided → use it with optional model (apply alias resolution)
    if let Some(tool_name) = tool {
        // Enforce tool enablement from user config
        if let Some(cfg) = config {
            cfg.enforce_tool_enabled(tool_name.as_str(), force_override_user_config)?;
        }
        let resolved_model = model.map(|m| {
            config
                .map(|cfg| cfg.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Enforce tier whitelist: tool must be in tiers; model name must match if provided
        if !force
            && !bypass_tier
            && let Some(cfg) = config
        {
            cfg.enforce_tier_whitelist(tool_name.as_str(), None)?;
            cfg.enforce_tier_model_name(
                tool_name.as_str(),
                model_name_for_tier_validation(resolved_model.as_deref()),
            )?;
        }
        // Catch known-incompatible model/tool combinations before spawning.
        if let Some(ref m) = resolved_model {
            let configured_default =
                config.and_then(|cfg| cfg.tool_default_model(tool_name.as_str()));
            model_compat::validate_tool_model_compat(tool_name, m, configured_default)?;
        }
        return Ok((tool_name, None, resolved_model));
    }

    // Case 3: no tool/model_spec; use tiers, or --force any enabled runtime.
    if force {
        for tool in csa_config::global::routing_candidate_tools() {
            let name = tool.as_str();
            let enabled = config.is_none_or(|cfg| cfg.is_tool_enabled(name));
            let extra_env = runtime_env_for_tool(name);
            if enabled
                && is_tool_runtime_available_for_config_with_env(
                    name,
                    config,
                    None,
                    extra_env.as_ref(),
                )
            {
                let tool_name = parse_tool_name(name)?;
                return Ok((tool_name, None, None));
            }
        }
        anyhow::bail!(
            "No installed and enabled tools found. Install at least one supported routing tool \
             (opencode, codex, claude-code) or check enabled status."
        );
    }

    if let Some(cfg) = config {
        let resolved_model = model.map(|m| {
            config
                .map(|c| c.resolve_alias(m))
                .unwrap_or_else(|| m.to_string())
        });
        // Round-robin rotation; write-restriction errors propagate.
        match csa_scheduler::resolve_tier_tool_rotated_with_catalog(
            cfg,
            model_catalog,
            "default",
            project_root,
            needs_edit,
        ) {
            Ok(Some((s, spec))) => {
                let extra_env = runtime_env_for_tool(&s);
                if is_tool_runtime_available_for_config_with_env(
                    &s,
                    Some(cfg),
                    Some(&spec),
                    extra_env.as_ref(),
                ) {
                    return Ok((parse_tool_name(&s)?, Some(spec), resolved_model));
                }
                tracing::warn!(
                    tool = %s,
                    model_spec = %spec,
                    "Skipping rotated tier candidate because the tool is not executable"
                );
            }
            Err(e) if csa_scheduler::is_no_writable_tier_tool_error(&e) => return Err(e),
            _ => {}
        }
        // Fallback: original non-rotating selection, but keep runtime
        // availability aligned with the rotated path.
        if let Some(resolution) = resolve_runtime_available_tier_fallback_with_catalog(
            cfg,
            global_config,
            model_catalog,
            "default",
            needs_edit,
        )? {
            return Ok((resolution.tool, Some(resolution.model_spec), resolved_model));
        }
    }

    // Minimal configs with empty tiers may pick any auto-selectable runtime.
    if let Some(cfg) = config
        && cfg.tiers.is_empty()
    {
        for tool in csa_config::global::routing_candidate_tools() {
            let name = tool.as_str();
            let extra_env = runtime_env_for_tool(name);
            if cfg.is_tool_auto_selectable(name)
                && is_tool_runtime_available_for_config_with_env(
                    name,
                    Some(cfg),
                    None,
                    extra_env.as_ref(),
                )
            {
                let tool_name = parse_tool_name(name)?;
                return Ok((tool_name, None, None));
            }
        }
    }

    // Case 4: no config, no tier, and no auto-selectable installed tool → error
    anyhow::bail!(
        "No tool specified and no tier-based or auto-selectable tool available. \
         Use --tool, run 'csa init --full' to configure tiers, or install a tool."
    )
}
