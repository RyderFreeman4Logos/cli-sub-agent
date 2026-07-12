#[test]
fn primary_writer_spec_seeds_run_without_model_selecting_flags_and_disables_tier_context() {
    let tmp = TempDir::new().expect("tempdir");
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let global_config = GlobalConfig::default();

    let spec = resolve_primary_writer_spec_for_run(
        RunModelSelectionFlags::default(),
        Some(&config),
        &global_config,
    )
    .expect("primary writer spec should apply");
    let resolution = resolve_tool_by_strategy(
        &ToolSelectionStrategy::AnyAvailable,
        Some(&spec),
        None,
        None, // thinking
        Some(&config),
        &global_config,
        tmp.path(),
        false,
        false,
        false,
        None,
        false,
    )
    .expect("primary writer spec should resolve as model-spec");

    assert_eq!(resolution.tool, ToolName::Codex);
    assert_eq!(resolution.model_spec.as_deref(), Some(spec.as_str()));
    assert!(
        resolution.resolved_tier_name.is_none(),
        "synthetic model-spec should still disable tier runtime context"
    );
}

#[test]
fn primary_writer_spec_prefers_project_over_global() {
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let mut global_config = GlobalConfig::default();
    global_config.preferences.primary_writer_spec =
        Some("claude-code/anthropic/default/xhigh".to_string());

    let spec = resolve_primary_writer_spec_for_run(
        RunModelSelectionFlags::default(),
        Some(&config),
        &global_config,
    );

    assert_eq!(spec.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn primary_writer_spec_is_suppressed_by_any_model_selecting_flag() {
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let global_config = GlobalConfig::default();
    let cases = [
        RunModelSelectionFlags {
            tool: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            auto_route: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            skill: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            model_spec: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            model: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            thinking: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            tier: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            hint_difficulty: true,
            ..Default::default()
        },
    ];

    for flags in cases {
        let spec = resolve_primary_writer_spec_for_run(flags, Some(&config), &global_config);
        assert!(
            spec.is_none(),
            "flags should suppress primary writer: {flags:?}"
        );
    }
}
