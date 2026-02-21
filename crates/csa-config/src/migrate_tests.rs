use super::*;
use crate::paths::XdgPathPair;
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[test]
fn test_version_ordering() {
    let v1 = Version::new(0, 12, 0);
    let v2 = Version::new(0, 12, 1);
    let v3 = Version::new(1, 0, 0);
    assert!(v1 < v2);
    assert!(v2 < v3);
}

#[test]
fn test_version_parse_and_display() {
    let v: Version = "1.2.3".parse().unwrap();
    assert_eq!(v, Version::new(1, 2, 3));
    assert_eq!(v.to_string(), "1.2.3");
}

#[test]
fn test_version_parse_invalid() {
    assert!("1.2".parse::<Version>().is_err());
    assert!("abc".parse::<Version>().is_err());
}

#[test]
fn test_registry_pending_filters_applied() {
    let mut registry = MigrationRegistry::new();
    registry.register(Migration {
        id: "0.12.0-rename-plans".to_string(),
        from_version: Version::new(0, 12, 0),
        to_version: Version::new(0, 12, 1),
        description: "Rename plan files".to_string(),
        steps: vec![],
    });
    registry.register(Migration {
        id: "0.12.1-update-config".to_string(),
        from_version: Version::new(0, 12, 1),
        to_version: Version::new(0, 13, 0),
        description: "Update config format".to_string(),
        steps: vec![],
    });

    let current = Version::new(0, 12, 0);
    let target = Version::new(0, 13, 0);

    // Nothing applied → both pending
    let pending = registry.pending(&current, &target, &[]);
    assert_eq!(pending.len(), 2);

    // First applied → only second pending
    let applied = vec!["0.12.0-rename-plans".to_string()];
    let pending = registry.pending(&current, &target, &applied);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "0.12.1-update-config");
}

#[test]
fn test_rename_file_step() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.txt"), "content").unwrap();

    let step = MigrationStep::RenameFile {
        from: PathBuf::from("old.txt"),
        to: PathBuf::from("new.txt"),
    };
    execute_step(&step, dir.path()).unwrap();

    assert!(!dir.path().join("old.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
        "content"
    );
}

#[test]
fn test_rename_file_idempotent() {
    let dir = TempDir::new().unwrap();
    // Source doesn't exist — should be a no-op
    let step = MigrationStep::RenameFile {
        from: PathBuf::from("missing.txt"),
        to: PathBuf::from("target.txt"),
    };
    execute_step(&step, dir.path()).unwrap();
}

#[test]
fn test_replace_in_file_step() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[plan]\nkey = \"old\"").unwrap();

    let step = MigrationStep::ReplaceInFile {
        path: PathBuf::from("config.toml"),
        old: "[plan]".to_string(),
        new: "[workflow]".to_string(),
    };
    execute_step(&step, dir.path()).unwrap();

    let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert_eq!(content, "[workflow]\nkey = \"old\"");
}

#[test]
fn test_replace_in_file_idempotent() {
    let dir = TempDir::new().unwrap();
    // File doesn't exist — no-op
    let step = MigrationStep::ReplaceInFile {
        path: PathBuf::from("missing.toml"),
        old: "old".to_string(),
        new: "new".to_string(),
    };
    execute_step(&step, dir.path()).unwrap();
}

#[test]
fn test_custom_step() {
    let dir = TempDir::new().unwrap();
    let step = MigrationStep::Custom {
        label: "create marker".to_string(),
        apply: Box::new(|root| {
            std::fs::write(root.join("marker.txt"), "migrated")?;
            Ok(())
        }),
    };
    execute_step(&step, dir.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("marker.txt")).unwrap(),
        "migrated"
    );
}

#[test]
fn test_execute_migration_runs_all_steps() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

    let migration = Migration {
        id: "test-migration".to_string(),
        from_version: Version::new(0, 1, 0),
        to_version: Version::new(0, 2, 0),
        description: "Test".to_string(),
        steps: vec![
            MigrationStep::RenameFile {
                from: PathBuf::from("a.txt"),
                to: PathBuf::from("b.txt"),
            },
            MigrationStep::ReplaceInFile {
                path: PathBuf::from("b.txt"),
                old: "hello".to_string(),
                new: "world".to_string(),
            },
        ],
    };
    execute_migration(&migration, dir.path()).unwrap();

    assert!(!dir.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "world"
    );
}

#[test]
fn test_registry_ordering() {
    let mut registry = MigrationRegistry::new();
    // Insert out of order
    registry.register(Migration {
        id: "second".to_string(),
        from_version: Version::new(0, 2, 0),
        to_version: Version::new(0, 3, 0),
        description: "Second".to_string(),
        steps: vec![],
    });
    registry.register(Migration {
        id: "first".to_string(),
        from_version: Version::new(0, 1, 0),
        to_version: Version::new(0, 2, 0),
        description: "First".to_string(),
        steps: vec![],
    });

    assert_eq!(registry.all()[0].id, "first");
    assert_eq!(registry.all()[1].id, "second");
}

// =======================================================================
// Plan → workflow migration tests
// =======================================================================

#[test]
fn test_default_registry_contains_plan_to_workflow() {
    let registry = default_registry();
    assert!(!registry.all().is_empty());
    assert_eq!(registry.all()[0].id, "0.1.2-plan-to-workflow");
}

#[test]
fn test_is_plan_table_header() {
    assert!(is_plan_table_header("[plan]"));
    assert!(is_plan_table_header("[[plan.steps]]"));
    assert!(is_plan_table_header("[[plan.variables]]"));
    assert!(is_plan_table_header("[plan.steps.on_fail]"));
    assert!(is_plan_table_header("[plan.steps.loop_var]"));

    // Non-table-header lines should not match.
    assert!(!is_plan_table_header("name = \"plan\""));
    assert!(!is_plan_table_header("# [plan]"));
    assert!(!is_plan_table_header("plan = true"));
    assert!(!is_plan_table_header("[workflow]"));
}

#[test]
fn test_replace_plan_in_header() {
    assert_eq!(replace_plan_in_header("[plan]"), "[workflow]");
    assert_eq!(
        replace_plan_in_header("[[plan.steps]]"),
        "[[workflow.steps]]"
    );
    assert_eq!(
        replace_plan_in_header("[plan.steps.on_fail]"),
        "[workflow.steps.on_fail]"
    );
    // Already renamed — no double replacement.
    assert_eq!(replace_plan_in_header("[workflow]"), "[workflow]");
}

#[test]
fn test_rename_plan_keys_in_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("workflow.toml");
    std::fs::write(
        &path,
        "[plan]\nname = \"test\"\n\n[[plan.steps]]\nid = 1\ntitle = \"Hello\"\nprompt = \"Hi\"\n",
    )
    .unwrap();

    rename_plan_keys_in_file(&path).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[workflow]"));
    assert!(content.contains("[[workflow.steps]]"));
    assert!(!content.contains("[plan]"));
    assert!(!content.contains("[[plan."));
    // Non-header content preserved.
    assert!(content.contains("name = \"test\""));
}

#[test]
fn test_rename_plan_keys_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("workflow.toml");
    let original = "[workflow]\nname = \"test\"\n\n[[workflow.steps]]\nid = 1\ntitle = \"S\"\nprompt = \"P\"\n";
    std::fs::write(&path, original).unwrap();

    rename_plan_keys_in_file(&path).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, original, "already-migrated file should not change");
}

#[test]
fn test_rename_plan_keys_preserves_non_plan_values() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("workflow.toml");
    // A file where "plan" appears in values, not just keys.
    let input = "[plan]\nname = \"my-plan\"\ndescription = \"This is a plan\"\n\n[[plan.steps]]\nid = 1\ntitle = \"Execute plan\"\nprompt = \"Run the plan\"\n";
    std::fs::write(&path, input).unwrap();

    rename_plan_keys_in_file(&path).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    // Table headers renamed.
    assert!(content.contains("[workflow]"));
    assert!(content.contains("[[workflow.steps]]"));
    // Values with "plan" preserved.
    assert!(content.contains("name = \"my-plan\""));
    assert!(content.contains("description = \"This is a plan\""));
    assert!(content.contains("title = \"Execute plan\""));
}

#[test]
fn test_rename_plan_keys_in_project_no_patterns_dir() {
    let dir = TempDir::new().unwrap();
    // No patterns/ directory — should be a no-op.
    rename_plan_keys_in_project(dir.path()).unwrap();
}

#[test]
fn test_rename_plan_keys_in_project_with_patterns() {
    let dir = TempDir::new().unwrap();
    let pattern_dir = dir.path().join("patterns").join("my-pattern");
    std::fs::create_dir_all(&pattern_dir).unwrap();

    let workflow = pattern_dir.join("workflow.toml");
    std::fs::write(
        &workflow,
        "[plan]\nname = \"test\"\n\n[[plan.variables]]\nname = \"X\"\n\n[[plan.steps]]\nid = 1\ntitle = \"S\"\nprompt = \"P\"\n",
    )
    .unwrap();

    rename_plan_keys_in_project(dir.path()).unwrap();

    let content = std::fs::read_to_string(&workflow).unwrap();
    assert!(content.contains("[workflow]"));
    assert!(content.contains("[[workflow.variables]]"));
    assert!(content.contains("[[workflow.steps]]"));
    assert!(!content.contains("[plan]"));
}

#[test]
fn test_rename_plan_keys_nested_tables() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("workflow.toml");
    let input = "\
[plan]
name = \"complex\"

[[plan.steps]]
id = 1
title = \"S\"
prompt = \"P\"

[plan.steps.on_fail]
retry = 3

[plan.steps.loop_var]
variable = \"item\"
collection = \"items\"
";
    std::fs::write(&path, input).unwrap();

    rename_plan_keys_in_file(&path).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[workflow]"));
    assert!(content.contains("[[workflow.steps]]"));
    assert!(content.contains("[workflow.steps.on_fail]"));
    assert!(content.contains("[workflow.steps.loop_var]"));
    assert!(!content.contains("[plan]"));
    assert!(!content.contains("[plan."));
}

// =======================================================================
// Full migration cycle integration tests
// =======================================================================

#[test]
fn test_full_migrate_cycle() {
    let dir = TempDir::new().unwrap();

    // Set up a test project with legacy [plan] format.
    let pattern_dir = dir.path().join("patterns").join("test-pattern");
    std::fs::create_dir_all(&pattern_dir).unwrap();
    std::fs::write(
        pattern_dir.join("workflow.toml"),
        "[plan]\nname = \"test\"\n\n[[plan.steps]]\nid = 1\ntitle = \"S\"\nprompt = \"P\"\n",
    )
    .unwrap();

    // Create a weave.lock at the old version.
    let lock = crate::WeaveLock::new("0.1.1", "0.1.1");
    lock.save(dir.path()).unwrap();

    // Get the default registry and run all pending migrations.
    let registry = default_registry();
    let current = Version::new(0, 1, 1);
    let target = Version::new(0, 1, 2);
    let pending = registry.pending(&current, &target, &[]);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "0.1.2-plan-to-workflow");

    // Execute.
    for m in &pending {
        execute_migration(m, dir.path()).unwrap();
    }

    // Verify workflow.toml was transformed.
    let content = std::fs::read_to_string(pattern_dir.join("workflow.toml")).unwrap();
    assert!(content.contains("[workflow]"));
    assert!(!content.contains("[plan]"));
}

#[test]
fn test_full_migrate_cycle_already_migrated_is_noop() {
    let dir = TempDir::new().unwrap();

    // Project already uses [workflow].
    let pattern_dir = dir.path().join("patterns").join("test-pattern");
    std::fs::create_dir_all(&pattern_dir).unwrap();
    let original = "[workflow]\nname = \"test\"\n\n[[workflow.steps]]\nid = 1\ntitle = \"S\"\nprompt = \"P\"\n";
    std::fs::write(pattern_dir.join("workflow.toml"), original).unwrap();

    let registry = default_registry();
    let current = Version::new(0, 1, 1);
    let target = Version::new(0, 1, 2);
    let pending = registry.pending(&current, &target, &[]);

    for m in &pending {
        execute_migration(m, dir.path()).unwrap();
    }

    // File should be unchanged.
    let content = std::fs::read_to_string(pattern_dir.join("workflow.toml")).unwrap();
    assert_eq!(content, original);
}

#[test]
fn test_full_migrate_cycle_skips_already_applied() {
    let registry = default_registry();
    let current = Version::new(0, 1, 1);
    let target = Version::new(0, 1, 2);

    // Mark the migration as already applied.
    let applied = vec!["0.1.2-plan-to-workflow".to_string()];
    let pending = registry.pending(&current, &target, &applied);
    assert!(
        pending.is_empty(),
        "already-applied migration should be skipped"
    );
}

#[test]
fn test_rename_file_creates_parent_dirs() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("src.txt"), "data").unwrap();

    let step = MigrationStep::RenameFile {
        from: PathBuf::from("src.txt"),
        to: PathBuf::from("nested/deep/dst.txt"),
    };
    execute_step(&step, dir.path()).unwrap();

    assert!(!dir.path().join("src.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/deep/dst.txt")).unwrap(),
        "data"
    );
}

#[test]
fn test_replace_in_file_no_match_is_noop() {
    let dir = TempDir::new().unwrap();
    let original = "nothing to replace here";
    std::fs::write(dir.path().join("file.txt"), original).unwrap();

    let step = MigrationStep::ReplaceInFile {
        path: PathBuf::from("file.txt"),
        old: "nonexistent".to_string(),
        new: "replacement".to_string(),
    };
    execute_step(&step, dir.path()).unwrap();

    let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
    assert_eq!(content, original);
}

#[test]
fn test_replace_in_file_multiple_occurrences() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("multi.txt"), "old old old").unwrap();

    let step = MigrationStep::ReplaceInFile {
        path: PathBuf::from("multi.txt"),
        old: "old".to_string(),
        new: "new".to_string(),
    };
    execute_step(&step, dir.path()).unwrap();

    let content = std::fs::read_to_string(dir.path().join("multi.txt")).unwrap();
    assert_eq!(content, "new new new");
}

#[test]
fn test_version_comparison_edge_cases() {
    // Same version.
    assert_eq!(Version::new(1, 0, 0), Version::new(1, 0, 0));

    // Major dominates.
    assert!(Version::new(2, 0, 0) > Version::new(1, 99, 99));

    // Minor dominates over patch.
    assert!(Version::new(0, 2, 0) > Version::new(0, 1, 99));

    // Patch comparison.
    assert!(Version::new(0, 0, 2) > Version::new(0, 0, 1));
}

#[test]
fn test_pending_empty_registry() {
    let registry = MigrationRegistry::new();
    let pending = registry.pending(&Version::new(0, 0, 0), &Version::new(99, 99, 99), &[]);
    assert!(pending.is_empty());
}

#[test]
fn test_pending_filters_by_version_range() {
    let mut registry = MigrationRegistry::new();
    registry.register(Migration {
        id: "early".to_string(),
        from_version: Version::new(0, 1, 0),
        to_version: Version::new(0, 2, 0),
        description: "Early".to_string(),
        steps: vec![],
    });
    registry.register(Migration {
        id: "late".to_string(),
        from_version: Version::new(1, 0, 0),
        to_version: Version::new(2, 0, 0),
        description: "Late".to_string(),
        steps: vec![],
    });

    // Only the early migration is in range 0.1.0 → 0.5.0.
    let pending = registry.pending(&Version::new(0, 1, 0), &Version::new(0, 5, 0), &[]);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "early");
}

#[test]
fn test_migration_step_debug_format() {
    let step = MigrationStep::RenameFile {
        from: PathBuf::from("a"),
        to: PathBuf::from("b"),
    };
    let debug = format!("{step:?}");
    assert!(debug.contains("RenameFile"));

    let step = MigrationStep::Custom {
        label: "custom-label".to_string(),
        apply: Box::new(|_| Ok(())),
    };
    let debug = format!("{step:?}");
    assert!(debug.contains("custom-label"));
}

#[test]
fn test_xdg_migration_legacy_to_new_with_symlink() {
    let dir = TempDir::new().unwrap();
    let admin_dir = dir.path().join("admin");
    let legacy = dir.path().join("csa-state");
    let new_path = dir.path().join("cli-sub-agent-state");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("state.toml"), "hello").unwrap();

    let pairs = vec![XdgPathPair {
        label: "state",
        new_path: new_path.clone(),
        legacy_path: legacy.clone(),
    }];

    migrate_xdg_paths_for_pairs(pairs, &admin_dir).unwrap();

    assert!(new_path.join("state.toml").exists());
    let meta = std::fs::symlink_metadata(&legacy).unwrap();
    assert!(meta.file_type().is_symlink());
    assert_eq!(std::fs::read_link(&legacy).unwrap(), new_path);
}

#[test]
fn test_xdg_migration_new_path_already_exists_noop_when_legacy_absent() {
    let dir = TempDir::new().unwrap();
    let admin_dir = dir.path().join("admin");
    let legacy = dir.path().join("csa-state");
    let new_path = dir.path().join("cli-sub-agent-state");
    std::fs::create_dir_all(&new_path).unwrap();
    std::fs::write(new_path.join("existing.txt"), "keep").unwrap();

    let pairs = vec![XdgPathPair {
        label: "state",
        new_path: new_path.clone(),
        legacy_path: legacy.clone(),
    }];

    migrate_xdg_paths_for_pairs(pairs, &admin_dir).unwrap();

    assert!(new_path.join("existing.txt").exists());
    assert!(!legacy.exists());
}

#[test]
fn test_xdg_migration_recovers_from_marker_and_rolls_back() {
    let dir = TempDir::new().unwrap();
    let admin_dir = dir.path().join("admin");
    std::fs::create_dir_all(&admin_dir).unwrap();

    let legacy = dir.path().join("csa-state");
    let new_path = dir.path().join("cli-sub-agent-state");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("state.toml"), "legacy").unwrap();
    std::fs::rename(&legacy, &new_path).unwrap();

    let marker = XdgMigrationMarker {
        operations: vec![XdgMigrationOperation::MoveLegacyToNew {
            legacy: legacy.clone(),
            new_path: new_path.clone(),
        }],
    };
    write_marker(&marker_path(&admin_dir), &marker).unwrap();

    recover_incomplete_xdg_migration(&admin_dir).unwrap();

    assert!(legacy.join("state.toml").exists());
    assert!(!new_path.exists());
    assert!(!marker_path(&admin_dir).exists());
}

#[cfg(unix)]
#[test]
fn test_xdg_migration_concurrent_flock_blocks_second_migration() {
    let dir = TempDir::new().unwrap();
    let admin_dir = dir.path().join("admin");
    std::fs::create_dir_all(&admin_dir).unwrap();

    let lock = GlobalMigrationLock::acquire(&admin_dir).unwrap();
    let (tx, rx) = mpsc::channel();
    let admin_clone = admin_dir.clone();

    let handle = thread::spawn(move || {
        let started = Instant::now();
        let _guard = GlobalMigrationLock::acquire(&admin_clone).unwrap();
        tx.send(started.elapsed()).unwrap();
    });

    thread::sleep(Duration::from_millis(200));
    assert!(
        rx.try_recv().is_err(),
        "second migration lock should still be blocked"
    );

    drop(lock);
    let elapsed = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        elapsed >= Duration::from_millis(150),
        "expected lock blocking, got {:?}",
        elapsed
    );

    handle.join().unwrap();
}
