use std::fs;
use std::path::PathBuf;

use csa_session::{
    MetaSessionState, SessionPhase, load_output_index, persist_structured_output, read_section,
};

fn fixture_path(relative: &str) -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    workspace_root.join("tests/fixtures").join(relative)
}

fn read_fixture_state(relative: &str) -> MetaSessionState {
    let path = fixture_path(relative);
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", path.display()));
    toml::from_str(&content)
        .unwrap_or_else(|err| panic!("failed to parse fixture {}: {err}", path.display()))
}

#[test]
fn test_parse_real_claude_state() {
    let state = read_fixture_state("claude/session-001/state.toml");

    assert_eq!(state.phase, SessionPhase::Active);
    assert!(state.tools.contains_key("claude-code"));
    assert_eq!(state.genealogy.depth, 0);
    assert!(state.genealogy.parent_session_id.is_none());
}

#[test]
fn test_parse_real_codex_state() {
    let state = read_fixture_state("codex/session-001/state.toml");

    assert_eq!(state.phase, SessionPhase::Active);
    assert!(state.tools.contains_key("codex"));
    assert_eq!(state.genealogy.depth, 0);
    assert!(state.genealogy.parent_session_id.is_none());
}

#[test]
fn test_output_section_markers() {
    let output_path = fixture_path("claude/session-001/output.log");
    let output = fs::read_to_string(&output_path)
        .unwrap_or_else(|err| panic!("failed to read output fixture {}: {err}", output_path.display()));

    let tmp = tempfile::tempdir().expect("create temp dir");
    let index = persist_structured_output(tmp.path(), &output).expect("persist structured output");

    assert!(
        index.sections.iter().any(|section| section.id == "summary"),
        "summary section should be parsed"
    );
    assert!(
        index.sections.iter().any(|section| section.id == "details"),
        "details section should be parsed"
    );

    let summary = read_section(tmp.path(), "summary")
        .expect("read summary section")
        .expect("summary section exists");
    assert!(summary.contains("辩论结果"));

    let loaded_index = load_output_index(tmp.path())
        .expect("load index")
        .expect("index exists");
    assert_eq!(loaded_index.sections.len(), index.sections.len());
}

#[test]
fn test_fixture_roundtrip() {
    for relative in ["claude/session-001/state.toml", "codex/session-001/state.toml"] {
        let state = read_fixture_state(relative);
        let encoded = toml::to_string_pretty(&state).expect("serialize state");
        let decoded: MetaSessionState = toml::from_str(&encoded).expect("deserialize state");

        let before = serde_json::to_value(&state).expect("state to json value");
        let after = serde_json::to_value(&decoded).expect("decoded to json value");
        assert_eq!(before, after, "roundtrip mismatch for {relative}");
    }
}
