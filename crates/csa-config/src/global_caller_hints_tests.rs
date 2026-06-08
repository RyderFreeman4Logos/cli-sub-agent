use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CallerHintsRoot {
    #[serde(default)]
    caller_hints: CallerHintsConfig,
}

#[test]
fn test_caller_hints_defaults_parse_when_section_omitted() {
    let config: CallerHintsRoot = toml::from_str("").unwrap();
    assert_eq!(
        config.caller_hints.codex_session_wait_yield_ms,
        DEFAULT_CODEX_SESSION_WAIT_YIELD_MS
    );
}

#[test]
fn test_caller_hints_codex_session_wait_yield_ms_parses_from_config() {
    let config: CallerHintsRoot = toml::from_str(
        r#"
[caller_hints]
codex_session_wait_yield_ms = 450000
"#,
    )
    .unwrap();
    assert_eq!(config.caller_hints.codex_session_wait_yield_ms, 450_000);
}

#[test]
fn test_resolve_codex_session_wait_yield_ms_uses_configured_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[caller_hints]
codex_session_wait_yield_ms = 450000
"#,
    )
    .unwrap();

    assert_eq!(
        GlobalConfig::resolve_codex_session_wait_yield_ms_from_path(Some(&path)),
        450_000
    );
}
