//! Overlay-after-writable regressions for migrated Hermes profile binds (#3148).

use super::*;

#[cfg(unix)]
#[test]
fn migrated_profile_databases_remain_writable_through_bwrap_overlays() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-profile-bwrap-write-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("direct")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles/nested")).unwrap();
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("direct/config.yaml"), "direct: true\n").unwrap();
    std::fs::write(
        hermes_home.join("profiles/nested/config.yaml"),
        "nested: true\n",
    )
    .unwrap();
    let direct_connection = live_sqlite_database(&hermes_home.join("direct/state.db"), "direct");
    let nested_connection =
        live_sqlite_database(&hermes_home.join("profiles/nested/state.db"), "nested");
    let flat_connection = live_sqlite_database(&hermes_home.join("state.flat.db"), "flat");

    let plan = hermes_plan(&hermes_home).expect("Hermes plan must migrate profile databases");
    drop((direct_connection, nested_connection, flat_connection));
    let args = command_args(&plan);
    let runtime = hermes_home.join(".csa-runtime");
    let layouts = [
        (
            "direct",
            hermes_home.join("direct/state.db"),
            runtime.join("direct/state.db"),
            "direct",
        ),
        (
            "nested",
            hermes_home.join("profiles/nested/state.db"),
            runtime.join("profiles/nested/state.db"),
            "nested",
        ),
        (
            "flat",
            hermes_home.join("state.flat.db"),
            runtime.join("state.flat.db"),
            "flat",
        ),
    ];

    for (profile, legacy_db, runtime_db, value) in &layouts {
        assert_eq!(
            crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile)),
            *runtime_db,
            "{profile} must resolve to the migrated DB used by xurl and recall"
        );
        let migrated = rusqlite::Connection::open(runtime_db).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            *value
        );
        assert!(
            legacy_db.exists(),
            "legacy {profile} DB must remain in place"
        );

        let requested_dir = legacy_db.parent().unwrap();
        let writable = plan
            .readable_paths
            .iter()
            .find(|path| path.writable_bind() && path.requested() == requested_dir)
            .expect("migrated profile must have a pinned writable bind");
        assert_eq!(
            writable.bind_source(),
            runtime_db.parent().unwrap(),
            "{profile} writes must bind the migrated runtime directory"
        );
        let writable_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--bind-fd" && window[2] == requested_dir.to_string_lossy().as_ref()
            })
            .expect("migrated profile must have a pinned writable bind");
        if *profile != "flat" {
            let config = requested_dir.join("config.yaml");
            let config_overlay = plan
                .readable_paths
                .iter()
                .find(|path| path.overrides_writable_mount() && path.requested() == config)
                .expect("profile configuration must remain visible read-only");
            let config_pos = args
                .windows(3)
                .position(|window| {
                    window[0] == "--ro-bind-fd"
                        && window[2] == config_overlay.requested().to_string_lossy().as_ref()
                })
                .expect("profile configuration must use a pinned read-only bind");
            assert!(
                writable_pos < config_pos,
                "profile configuration must be restored read-only after its writable directory bind: {args:?}"
            );
        }
        if requested_dir == hermes_home {
            assert_eq!(profile, &"flat");
            assert_eq!(writable.bind_source(), &runtime);
            assert!(
                args.windows(3).any(|window| {
                    window[0] == "--bind-fd" && window[2] == hermes_home.to_string_lossy().as_ref()
                }),
                "flat profile must be covered by the pinned writable Hermes home bind: {args:?}"
            );
            continue;
        }
        let overlay = plan
            .readable_paths
            .iter()
            .filter(|path| {
                path.overrides_writable_mount() && requested_dir.starts_with(path.requested())
            })
            .max_by_key(|path| path.requested().components().count())
            .expect("migrated profile must remain under a read-only overlay");
        let overlay_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind-fd"
                    && window[2] == overlay.requested().to_string_lossy().as_ref()
            })
            .expect("profile overlay must use its pinned read-only bind");
        assert!(
            overlay_pos < writable_pos,
            "writable {profile} bind must follow its parent overlay: {args:?}"
        );
    }
}
