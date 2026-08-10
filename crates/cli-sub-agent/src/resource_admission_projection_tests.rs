use super::*;

#[test]
fn writer_default_projection_is_raised_to_its_soft_limit_floor() {
    let config: ProjectConfig =
        toml::from_str("[resources]\nsoft_limit_percent = 90\nmin_free_memory_mb = 100\n")
            .expect("config should parse");

    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            Some("run"),
            Some(&config),
            "codex",
            RunResourceOverrides::absent(),
            10_100,
        ),
        10_000,
        "a default writer projection must not fall below its 90% soft-limit floor"
    );
    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            Some("run"),
            Some(&config),
            "codex",
            RunResourceOverrides::absent(),
            8_377,
        ),
        10_000,
        "the host gate must reject an impossible floor instead of selecting a sub-floor default"
    );
}
