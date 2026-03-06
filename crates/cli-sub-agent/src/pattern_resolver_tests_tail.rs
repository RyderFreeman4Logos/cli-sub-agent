use super::*;

#[test]
fn config_cascade_full_dir_fork_takes_highest_priority() {
    let tmp = TempDir::new().unwrap();
    let user_cfg = TempDir::new().unwrap();

    make_pattern_dir(
        tmp.path(),
        ".csa/patterns/review",
        "review",
        "# Fork Review",
        Some(
            r#"
[skill]
name = "review"

[agent]
tier = "tier-fork"
max_turns = 99
"#,
        ),
    );

    make_pattern_dir(
        tmp.path(),
        "patterns/review",
        "review",
        "# Repo Review",
        Some(
            r#"
[skill]
name = "review"

[agent]
tier = "tier-repo"
max_turns = 10
"#,
        ),
    );

    write_overlay(
        &user_cfg.path().join("patterns/review.toml"),
        r#"
[agent]
token_budget = 55555
"#,
    );

    let resolved = resolve_pattern("review", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("Fork Review"));

    let config = load_skill_config_with_user_dir(
        &tmp.path().join(".csa/patterns/review"),
        "review",
        tmp.path(),
        Some(user_cfg.path()),
    )
    .unwrap()
    .unwrap();

    let agent = config.agent.unwrap();
    assert_eq!(agent.tier.as_deref(), Some("tier-fork"));
    assert_eq!(agent.max_turns, Some(99));
    assert_eq!(agent.token_budget, Some(55555));
}

#[test]
fn config_cascade_overlay_only_no_package_base() {
    let tmp = TempDir::new().unwrap();

    make_pattern_dir(tmp.path(), "patterns/bare", "bare", "# Bare", None);

    write_overlay(
        &tmp.path().join(".csa/patterns/bare.toml"),
        r#"
[skill]
name = "bare"

[agent]
tier = "tier-1"
max_turns = 42
"#,
    );

    let config = load_skill_config_with_user_dir(
        &tmp.path().join("patterns/bare"),
        "bare",
        tmp.path(),
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(config.skill.name, "bare");
    assert_eq!(config.agent.unwrap().max_turns, Some(42));
}

#[test]
fn config_cascade_no_overlays_returns_base() {
    let tmp = TempDir::new().unwrap();

    make_pattern_dir(
        tmp.path(),
        "patterns/solo",
        "solo",
        "# Solo",
        Some(
            r#"
[skill]
name = "solo"

[agent]
max_turns = 7
"#,
        ),
    );

    let config = load_skill_config_with_user_dir(
        &tmp.path().join("patterns/solo"),
        "solo",
        tmp.path(),
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(config.skill.name, "solo");
    assert_eq!(config.agent.unwrap().max_turns, Some(7));
}

#[test]
fn config_cascade_no_config_anywhere_returns_none() {
    let tmp = TempDir::new().unwrap();

    make_pattern_dir(tmp.path(), "patterns/empty", "empty", "# Empty", None);

    let config = load_skill_config_with_user_dir(
        &tmp.path().join("patterns/empty"),
        "empty",
        tmp.path(),
        None,
    )
    .unwrap();

    assert!(config.is_none());
}
