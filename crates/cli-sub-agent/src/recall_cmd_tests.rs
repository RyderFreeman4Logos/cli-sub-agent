use std::path::Path;

use super::*;

fn make_entry(sid: &str) -> RecallHistoryEntry {
    RecallHistoryEntry {
        ts: "2026-05-08T17:48:14Z".to_string(),
        sid: sid.to_string(),
        project: "/tmp/project".to_string(),
        provider: "claude".to_string(),
    }
}

fn make_entry_with_project_and_provider(
    sid: &str,
    project: &str,
    provider: &str,
) -> RecallHistoryEntry {
    RecallHistoryEntry {
        ts: "2026-05-08T17:48:14Z".to_string(),
        sid: sid.to_string(),
        project: project.to_string(),
        provider: provider.to_string(),
    }
}

#[test]
fn append_history_entry_writes_jsonl_line() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

    let appended = append_history_entry(&history_path, &make_entry("sid-1")).expect("append");
    assert!(appended, "first append must write a line");

    let entries = load_history_entries(&history_path).expect("load");
    assert_eq!(entries, vec![make_entry("sid-1")]);
}

#[test]
fn append_history_entry_skips_recent_duplicate_sid() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

    append_history_entry(&history_path, &make_entry("sid-1")).expect("first append");
    let appended =
        append_history_entry(&history_path, &make_entry("sid-1")).expect("second append");

    assert!(!appended, "duplicate sid in recent window must be skipped");
    let entries = load_history_entries(&history_path).expect("load");
    assert_eq!(
        entries.len(),
        1,
        "duplicate append must not add a second line"
    );
}

#[test]
fn append_history_entry_allows_duplicate_outside_recent_window() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

    append_history_entry(&history_path, &make_entry("sid-1")).expect("first append");
    for index in 0..RECENT_DEDUP_WINDOW {
        let sid = format!("sid-{index}-other");
        append_history_entry(&history_path, &make_entry(&sid)).expect("filler append");
    }

    let appended = append_history_entry(&history_path, &make_entry("sid-1")).expect("late append");
    assert!(
        appended,
        "sid older than the dedup window must be recorded again"
    );
}

#[test]
fn output_guard_triggers_at_threshold() {
    let content = "x".repeat(OUTPUT_GUARD_BYTES);
    let message = output_guard_message("sid-1", &content).expect("guard");
    assert!(message.contains("OUTPUT_TOO_LARGE"));
    assert!(message.contains("csa recall read sid-1 | tail -100"));
}

#[test]
fn output_guard_allows_small_output() {
    let content = "x".repeat(OUTPUT_GUARD_BYTES - 1);
    assert!(output_guard_message("sid-1", &content).is_none());
}

#[test]
fn matching_ranges_merges_overlapping_context() {
    let lines = vec!["0", "match one", "2", "match two", "4"];
    let ranges = matching_ranges(&lines, "match", 1);
    assert_eq!(ranges, vec![(0, 4)]);
}

#[test]
fn recall_allowed_only_at_main_agent_depth() {
    assert!(
        recall_allowed_at_depth(0),
        "main agent (depth=0) must be recorded"
    );
    assert!(
        !recall_allowed_at_depth(1),
        "depth=1 child session must not be recorded"
    );
    assert!(
        !recall_allowed_at_depth(5),
        "deeply nested child (depth=5) must not be recorded"
    );
}

#[test]
fn thread_belongs_to_matching_project_claude() {
    let source = "/home/obj/.claude/projects/-home-obj-project-github-user-repo/abc.jsonl";
    let root = Path::new("/home/obj/project/github/user/repo");
    assert!(thread_belongs_to_project(
        source,
        root,
        xurl_core::ProviderKind::Claude
    ));
}

#[test]
fn thread_rejects_different_project_claude() {
    let source = "/home/obj/.claude/projects/-home-obj-project-github-user-other/abc.jsonl";
    let root = Path::new("/home/obj/project/github/user/repo");
    assert!(!thread_belongs_to_project(
        source,
        root,
        xurl_core::ProviderKind::Claude
    ));
}

#[test]
fn thread_belongs_to_project_codex_always_true() {
    let source = "/home/obj/.codex/sessions/2026/05/16/rollout-abc.jsonl";
    let root = Path::new("/home/obj/project/github/user/repo");
    assert!(
        thread_belongs_to_project(source, root, xurl_core::ProviderKind::Codex),
        "codex sessions don't encode project; always pass ownership check"
    );
}

#[test]
fn thread_belongs_to_project_gemini_always_true() {
    let source = "/home/obj/.gemini/history/session-abc.jsonl";
    let root = Path::new("/home/obj/project/github/user/repo");
    assert!(
        thread_belongs_to_project(source, root, xurl_core::ProviderKind::Gemini),
        "gemini sessions don't encode project; always pass ownership check"
    );
}

#[test]
fn latest_history_entry_returns_last_from_filtered_list() {
    let entry1 = make_entry_with_project_and_provider("sid-1", "/project/a", "claude");
    let entry2 = make_entry_with_project_and_provider("sid-2", "/project/b", "codex");
    let entry3 = make_entry_with_project_and_provider("sid-3", "/project/a", "gemini");

    let entries = vec![&entry1, &entry2, &entry3];

    let result = latest_history_entry(&entries).expect("latest");
    assert_eq!(result.sid, "sid-3", "latest should be the last entry");
}

#[test]
fn latest_history_entry_returns_none_for_empty_list() {
    let entries: Vec<&RecallHistoryEntry> = vec![];
    assert!(
        latest_history_entry(&entries).is_none(),
        "empty list should return None"
    );
}
