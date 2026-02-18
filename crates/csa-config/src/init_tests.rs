use super::*;
use tempfile::tempdir;

#[test]
fn test_init_project_creates_config() {
    let dir = tempdir().unwrap();
    let config = init_project(dir.path(), true, false).unwrap();

    assert!(!config.project.name.is_empty());
    assert_eq!(config.project.max_recursion_depth, 5);
    assert!(config.resources.min_free_memory_mb > 0);
    assert!(config.resources.idle_timeout_seconds > 0);

    // Config file should exist
    let config_path = ProjectConfig::config_path(dir.path());
    assert!(config_path.exists());
}

#[test]
fn test_init_project_fails_if_already_exists() {
    let dir = tempdir().unwrap();
    init_project(dir.path(), true, false).unwrap();

    // Second init should fail
    let result = init_project(dir.path(), true, false);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[test]
fn test_update_gitignore_adds_csa() {
    let dir = tempdir().unwrap();
    let gitignore_path = dir.path().join(".gitignore");

    // Create existing .gitignore
    std::fs::write(&gitignore_path, "target/\n*.log\n").unwrap();

    update_gitignore(dir.path()).unwrap();

    let content = std::fs::read_to_string(&gitignore_path).unwrap();
    assert!(content.contains(".csa/"));
    assert!(content.contains("target/"));
    assert!(content.contains("*.log"));
}

#[test]
fn test_update_gitignore_does_not_duplicate() {
    let dir = tempdir().unwrap();
    let gitignore_path = dir.path().join(".gitignore");

    // Create .gitignore with .csa/ already present
    std::fs::write(&gitignore_path, "target/\n.csa/\n*.log\n").unwrap();

    update_gitignore(dir.path()).unwrap();

    let content = std::fs::read_to_string(&gitignore_path).unwrap();
    let csa_count = content
        .lines()
        .filter(|line| line.trim() == ".csa/")
        .count();
    assert_eq!(csa_count, 1, "Should not duplicate .csa/ entry");
}

#[test]
fn test_update_gitignore_creates_if_missing() {
    let dir = tempdir().unwrap();
    let gitignore_path = dir.path().join(".gitignore");

    // No .gitignore exists
    assert!(!gitignore_path.exists());

    update_gitignore(dir.path()).unwrap();

    // Should now exist with .csa/ entry
    assert!(gitignore_path.exists());
    let content = std::fs::read_to_string(&gitignore_path).unwrap();
    assert!(content.contains(".csa/"));
}

#[test]
fn test_detect_installed_tools() {
    // This test just ensures the function runs without crashing
    let tools = detect_installed_tools();
    // We can't assert specific tools since it depends on the system
    // but we can verify the function returns valid tool names
    for tool in &tools {
        assert!(["gemini-cli", "opencode", "codex", "claude-code"].contains(tool));
    }
}

#[test]
fn test_smart_tiers_with_multiple_tools() {
    // Simulate a system with codex, gemini-cli, and claude-code installed
    let installed = vec!["codex", "gemini-cli", "claude-code"];
    let tiers = build_smart_tiers(&installed, &[]);

    // Verify all tiers are created
    assert_eq!(tiers.len(), 3);
    assert!(tiers.contains_key("tier-1-quick"));
    assert!(tiers.contains_key("tier-2-standard"));
    assert!(tiers.contains_key("tier-3-complex"));

    // tier-1-quick should prefer gemini-cli flash (fast, cheap)
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert_eq!(tier1.models.len(), 1);
    assert_eq!(
        tier1.models[0],
        "gemini-cli/google/gemini-3-flash-preview/xhigh"
    );

    // tier-2-standard should prefer codex sonnet (balanced)
    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert_eq!(tier2.models.len(), 1);
    assert_eq!(
        tier2.models[0],
        "codex/anthropic/claude-sonnet-4-5-20250929/default"
    );

    // tier-3-complex should prefer claude-code opus (deep reasoning)
    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert_eq!(tier3.models.len(), 1);
    assert_eq!(
        tier3.models[0],
        "claude-code/anthropic/claude-opus-4-6/default"
    );

    // Verify tier diversity: different tiers use different tools
    let tier1_tool = tier1.models[0].split('/').next().unwrap();
    let tier2_tool = tier2.models[0].split('/').next().unwrap();
    let tier3_tool = tier3.models[0].split('/').next().unwrap();

    assert_ne!(
        tier1_tool, tier2_tool,
        "tier-1 and tier-2 should use different tools"
    );
    assert_ne!(
        tier2_tool, tier3_tool,
        "tier-2 and tier-3 should use different tools"
    );
}

#[test]
fn test_smart_tiers_with_only_gemini() {
    // Simulate a system with only gemini-cli installed
    let installed = vec!["gemini-cli"];
    let tiers = build_smart_tiers(&installed, &[]);

    // Verify all tiers are created
    assert_eq!(tiers.len(), 3);

    // All tiers should use gemini-cli
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(tier1.models[0].starts_with("gemini-cli/"));

    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert!(tier2.models[0].starts_with("gemini-cli/"));

    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert!(tier3.models[0].starts_with("gemini-cli/"));

    // tier-1 should use flash variant
    assert!(tier1.models[0].contains("flash"));

    // tier-2 and tier-3 should use pro variant
    assert!(tier2.models[0].contains("pro"));
    assert!(tier3.models[0].contains("pro"));
}

#[test]
fn test_smart_tiers_with_no_tools() {
    // Simulate a system with no tools installed
    let installed: Vec<&str> = vec![];
    let tiers = build_smart_tiers(&installed, &[]);

    // Should still create all tiers with gemini-cli fallback
    assert_eq!(tiers.len(), 3);

    // All should fallback to gemini-cli
    for tier_name in &["tier-1-quick", "tier-2-standard", "tier-3-complex"] {
        let tier = tiers.get(*tier_name).unwrap();
        assert!(
            tier.models[0].starts_with("gemini-cli/"),
            "Tier {} should fallback to gemini-cli",
            tier_name
        );
    }
}

#[test]
fn test_init_project_creates_default_tier_mapping() {
    let dir = tempdir().unwrap();
    let config = init_project(dir.path(), true, false).unwrap();

    // Verify 'default' mapping exists
    assert!(
        config.tier_mapping.contains_key("default"),
        "'default' tier mapping should exist"
    );
    assert_eq!(
        config.tier_mapping.get("default").unwrap(),
        "tier-2-standard",
        "'default' should map to 'tier-2-standard'"
    );
}

// --- New tests below ---

#[test]
fn test_build_smart_tiers_with_only_codex() {
    let installed = vec!["codex"];
    let tiers = build_smart_tiers(&installed, &[]);

    assert_eq!(tiers.len(), 3);

    // tier-1-quick: codex (no gemini, so codex is first fallback)
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(tier1.models[0].starts_with("codex/"));
    assert!(tier1.models[0].contains("sonnet"));

    // tier-2-standard: codex (preferred for standard)
    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert!(tier2.models[0].starts_with("codex/"));
    assert!(tier2.models[0].contains("sonnet"));

    // tier-3-complex: codex (no claude-code, so codex is first fallback)
    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert!(tier3.models[0].starts_with("codex/"));
    assert!(tier3.models[0].contains("opus"));
}

#[test]
fn test_build_smart_tiers_with_only_claude_code() {
    let installed = vec!["claude-code"];
    let tiers = build_smart_tiers(&installed, &[]);

    assert_eq!(tiers.len(), 3);

    // tier-1-quick: claude-code sonnet (last fallback for tier1)
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(tier1.models[0].starts_with("claude-code/"));
    assert!(tier1.models[0].contains("sonnet"));

    // tier-2-standard: claude-code sonnet
    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert!(tier2.models[0].starts_with("claude-code/"));
    assert!(tier2.models[0].contains("sonnet"));

    // tier-3-complex: claude-code opus (preferred for complex)
    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert!(tier3.models[0].starts_with("claude-code/"));
    assert!(tier3.models[0].contains("opus"));
}

#[test]
fn test_build_smart_tiers_with_only_opencode() {
    let installed = vec!["opencode"];
    let tiers = build_smart_tiers(&installed, &[]);

    assert_eq!(tiers.len(), 3);

    // tier-1-quick: opencode sonnet
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(tier1.models[0].starts_with("opencode/"));
    assert!(tier1.models[0].contains("sonnet"));

    // tier-2-standard: opencode sonnet
    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert!(tier2.models[0].starts_with("opencode/"));

    // tier-3-complex: opencode opus
    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert!(tier3.models[0].starts_with("opencode/"));
    assert!(tier3.models[0].contains("opus"));
}

#[test]
fn test_build_smart_tiers_with_gemini_and_codex() {
    let installed = vec!["gemini-cli", "codex"];
    let tiers = build_smart_tiers(&installed, &[]);

    // tier-1-quick should use gemini flash (fastest/cheapest)
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(tier1.models[0].starts_with("gemini-cli/"));
    assert!(tier1.models[0].contains("flash"));

    // tier-2-standard should prefer codex
    let tier2 = tiers.get("tier-2-standard").unwrap();
    assert!(tier2.models[0].starts_with("codex/"));

    // tier-3-complex should use codex opus (no claude-code)
    let tier3 = tiers.get("tier-3-complex").unwrap();
    assert!(tier3.models[0].starts_with("codex/"));
    assert!(tier3.models[0].contains("opus"));
}

#[test]
fn test_build_smart_tiers_each_tier_has_exactly_one_model() {
    let installed = vec!["gemini-cli", "codex", "claude-code", "opencode"];
    let tiers = build_smart_tiers(&installed, &[]);

    for (name, tier) in &tiers {
        assert_eq!(
            tier.models.len(),
            1,
            "Tier '{}' should have exactly one model, got {}",
            name,
            tier.models.len()
        );
    }
}

#[test]
fn test_build_smart_tiers_descriptions_not_empty() {
    let installed = vec!["gemini-cli"];
    let tiers = build_smart_tiers(&installed, &[]);

    for (name, tier) in &tiers {
        assert!(
            !tier.description.is_empty(),
            "Tier '{}' should have a non-empty description",
            name
        );
    }
}

#[test]
fn test_default_tier_mapping_completeness() {
    let mapping = default_tier_mapping();

    // Must have a 'default' entry
    assert!(mapping.contains_key("default"));

    // Known expected task types
    let expected_keys = [
        "default",
        "security_audit",
        "architecture_design",
        "code_review",
        "feature_implementation",
        "bug_fix",
        "documentation",
        "quick_question",
    ];
    for key in &expected_keys {
        assert!(
            mapping.contains_key(*key),
            "tier_mapping should contain '{}'",
            key
        );
    }

    // All mapped tiers should reference valid tier names
    let valid_tiers = ["tier-1-quick", "tier-2-standard", "tier-3-complex"];
    for (task, tier) in &mapping {
        assert!(
            valid_tiers.contains(&tier.as_str()),
            "task '{}' maps to unknown tier '{}'",
            task,
            tier
        );
    }
}

#[test]
fn test_detect_installed_tools_with_mock_path() {
    // Create a tempdir with mock executables and pass as custom paths
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Create mock "gemini" and "codex" executables
    let gemini_path = bin_dir.join("gemini");
    std::fs::write(&gemini_path, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gemini_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let codex_path = bin_dir.join("codex");
    std::fs::write(&codex_path, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&codex_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Use detect_installed_tools_in_paths directly — no global PATH mutation
    let custom_path = std::ffi::OsStr::new(bin_dir.to_str().unwrap());
    let tools = detect_installed_tools_in_paths(custom_path);

    // Should detect gemini-cli and codex (mapped from "gemini" and "codex" executables)
    assert!(
        tools.contains(&"gemini-cli"),
        "Should detect gemini-cli from mock 'gemini' executable"
    );
    assert!(
        tools.contains(&"codex"),
        "Should detect codex from mock 'codex' executable"
    );
    // Should NOT detect tools we didn't mock
    assert!(
        !tools.contains(&"claude-code"),
        "Should not detect claude-code without mock"
    );
    assert!(
        !tools.contains(&"opencode"),
        "Should not detect opencode without mock"
    );
}

#[test]
fn test_detect_installed_tools_with_empty_path() {
    // Use a non-existent path so no tools are found — no global PATH mutation
    let custom_path = std::ffi::OsStr::new("/nonexistent_path_for_test");
    let tools = detect_installed_tools_in_paths(custom_path);

    assert!(
        tools.is_empty(),
        "Should detect no tools with empty/invalid PATH"
    );
}

#[test]
fn test_init_project_minimal_mode() {
    let dir = tempdir().unwrap();
    let config = init_project(dir.path(), true, true).unwrap();

    // Minimal mode should still have project name
    assert!(!config.project.name.is_empty());

    // Minimal mode: empty tools/tiers/tier_mapping (rely on global config / defaults)
    assert!(config.tools.is_empty(), "minimal should have no tools");
    assert!(config.tiers.is_empty(), "minimal should have no tiers");
    assert!(
        config.tier_mapping.is_empty(),
        "minimal should have no tier_mapping"
    );

    // Verify the serialized TOML only contains [project] section
    let config_path = ProjectConfig::config_path(dir.path());
    assert!(config_path.exists());
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !content.contains("[tools."),
        "minimal config should not contain [tools.*] sections"
    );
    assert!(
        !content.contains("[[tiers"),
        "minimal config should not contain tiers"
    );
    assert!(
        !content.contains("[tier_mapping]"),
        "minimal config should not contain [tier_mapping]"
    );
    assert!(
        !content.contains("[resources]"),
        "minimal config should not contain [resources]"
    );
}

#[test]
fn test_init_project_uses_directory_name() {
    let dir = tempdir().unwrap();
    let config = init_project(dir.path(), true, false).unwrap();

    // Project name should be derived from directory name
    let expected_name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(config.project.name, expected_name);
}

#[test]
fn test_init_project_creates_gitignore_with_csa() {
    let dir = tempdir().unwrap();
    init_project(dir.path(), true, false).unwrap();

    let gitignore_path = dir.path().join(".gitignore");
    assert!(gitignore_path.exists());
    let content = std::fs::read_to_string(&gitignore_path).unwrap();
    assert!(content.contains(".csa/"));
}

#[test]
fn test_update_gitignore_no_trailing_newline() {
    let dir = tempdir().unwrap();
    let gitignore_path = dir.path().join(".gitignore");

    // Write file without trailing newline
    std::fs::write(&gitignore_path, "target/").unwrap();

    update_gitignore(dir.path()).unwrap();

    let content = std::fs::read_to_string(&gitignore_path).unwrap();
    // Should have added a newline before .csa/
    assert!(content.contains("target/\n.csa/"));
}

#[test]
fn test_build_smart_tiers_skips_globally_disabled() {
    // gemini-cli and codex installed, but gemini-cli is globally disabled.
    let installed = vec!["gemini-cli", "codex", "claude-code"];
    let disabled = vec!["gemini-cli".to_string()];
    let tiers = build_smart_tiers(&installed, &disabled);

    // No tier should contain gemini-cli
    for (name, tier) in &tiers {
        for model in &tier.models {
            assert!(
                !model.starts_with("gemini-cli/"),
                "tier '{}' should not contain globally-disabled gemini-cli, found: {}",
                name,
                model
            );
        }
    }

    // tier-1-quick should fall back to codex (next preference after gemini-cli)
    let tier1 = tiers.get("tier-1-quick").unwrap();
    assert!(
        tier1.models[0].starts_with("codex/"),
        "tier-1-quick should use codex when gemini-cli is disabled"
    );
}

#[test]
fn test_build_smart_tiers_all_disabled() {
    // All tools installed but all globally disabled.
    let installed = vec!["gemini-cli", "codex", "claude-code"];
    let disabled = vec![
        "gemini-cli".to_string(),
        "codex".to_string(),
        "claude-code".to_string(),
    ];
    let tiers = build_smart_tiers(&installed, &disabled);

    // Should fall back to gemini-cli defaults (existing behavior for no tools)
    assert_eq!(tiers.len(), 3);
}
