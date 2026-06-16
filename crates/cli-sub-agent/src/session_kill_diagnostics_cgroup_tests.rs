use super::*;
use std::path::PathBuf;

#[test]
fn parses_cgroup_memory_events() {
    let events = parse_memory_events("low 2\nhigh 3\nmax 4\noom 1\noom_kill 1\n");

    assert_eq!(events.oom, 1);
    assert_eq!(events.oom_kill, 1);
    assert!(events.has_oom_event());
    assert!(events.has_oom_kill_event());
}

#[test]
fn cgroup_candidates_include_systemd_control_group_memory_events_path_first() {
    let scope = "csa-codex-01KV4HXVYPG7N5Z9VHG7JYWSBT.scope";
    let control_group = "/user.slice/user-1000.slice/user@1000.service/app.slice/app-csa.slice/\
         csa-codex-01KV4HXVYPG7N5Z9VHG7JYWSBT.scope";

    let candidates =
        cgroup_memory_event_candidates_from_parts(scope, Some(1000), Some(control_group));

    assert_eq!(
        candidates.first(),
        Some(&PathBuf::from(
            "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/\
             app-csa.slice/csa-codex-01KV4HXVYPG7N5Z9VHG7JYWSBT.scope/memory.events"
        ))
    );
    assert!(candidates.iter().any(|path| path
        == &PathBuf::from(
            "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/\
         csa-codex-01KV4HXVYPG7N5Z9VHG7JYWSBT.scope/memory.events"
        )));
}

#[test]
fn systemd_control_group_memory_events_path_rejects_empty_root_and_traversal() {
    for raw in ["", "\n", "/", "../evil.scope", "/user.slice/../evil.scope"] {
        assert!(
            systemd_control_group_memory_events_path(raw).is_none(),
            "invalid ControlGroup must not produce a host path: {raw:?}"
        );
    }
}
