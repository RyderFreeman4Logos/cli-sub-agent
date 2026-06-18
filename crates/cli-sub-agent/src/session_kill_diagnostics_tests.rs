use super::child_timeout::{
    ChildTimeoutKind, ChildTimeoutProvenance, detect_child_timeout_provenance,
};
use super::*;
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};

#[test]
fn parses_meminfo_available_and_total() {
    let meminfo = parse_meminfo(
        "\
MemTotal:       16384000 kB
MemFree:         1000000 kB
MemAvailable:    900000 kB
",
    )
    .expect("meminfo should parse");

    assert_eq!(meminfo.total_kb, 16_384_000);
    assert_eq!(meminfo.available_kb, 900_000);
    assert!(meminfo.available_below_ten_percent());
    assert!(!meminfo.available_below_five_percent());
}

#[test]
fn classifies_signal_exit_under_possible_memory_pressure() {
    let diagnostic = diagnose_signal_kill_with(143, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 999,
        }),
        earlyoom_running: false,
        cgroup_memory_events: None,
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::PossibleMemoryPressure);
    assert!(
        diagnostic
            .stderr_line()
            .expect("memory pressure should render")
            .contains("MemAvailable: 0 MB / MemTotal: 9 MB")
    );
}

#[test]
fn classifies_signal_exit_under_strong_memory_pressure() {
    let diagnostic = diagnose_signal_kill_with(143, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 499,
        }),
        earlyoom_running: false,
        cgroup_memory_events: None,
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::MemoryPressure);
    assert!(
        diagnostic
            .stderr_line()
            .expect("memory pressure should render")
            .contains("memory pressure")
    );
}

#[test]
fn classifies_unknown_when_earlyoom_runs_without_memory_pressure() {
    let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 5_000,
        }),
        earlyoom_running: true,
        cgroup_memory_events: None,
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    assert!(
        diagnostic
            .stderr_line()
            .expect("unknown signal should render checked evidence")
            .contains("earlyoom running")
    );
}

#[test]
fn classifies_earlyoom_when_daemon_runs_with_strong_memory_pressure() {
    let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 499,
        }),
        earlyoom_running: true,
        cgroup_memory_events: None,
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::Earlyoom);
    assert!(
        diagnostic
            .stderr_line()
            .expect("earlyoom should render")
            .contains("earlyoom running")
    );
}

#[test]
fn classifies_earlyoom_when_daemon_runs_with_cgroup_oom_kill() {
    let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 5_000,
        }),
        earlyoom_running: true,
        cgroup_memory_events: Some(CgroupMemoryEvents {
            oom: 0,
            oom_kill: 1,
        }),
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::Earlyoom);
}

#[test]
fn does_not_classify_earlyoom_from_oom_without_oom_kill() {
    let diagnostic = diagnose_signal_kill_with(137, None, || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 10_000,
            available_kb: 5_000,
        }),
        earlyoom_running: true,
        cgroup_memory_events: Some(CgroupMemoryEvents {
            oom: 1,
            oom_kill: 0,
        }),
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    assert!(
        diagnostic
            .stderr_line()
            .expect("unknown signal should render checked evidence")
            .contains("cgroup memory.events oom=1 oom_kill=0")
    );
}

#[test]
fn csa_timeout_terminal_reasons_take_precedence_over_earlyoom() {
    for terminal_reason in ["timeout", "idle_timeout", "initial_response_timeout"] {
        let called = Cell::new(false);
        let diagnostic = diagnose_signal_kill_with(137, Some(terminal_reason), || {
            called.set(true);
            KillSignalObservations {
                meminfo: Some(MemInfo {
                    total_kb: 10_000,
                    available_kb: 999,
                }),
                earlyoom_running: true,
                cgroup_memory_events: Some(CgroupMemoryEvents {
                    oom: 1,
                    oom_kill: 1,
                }),
            }
        })
        .expect("signal exit should produce diagnostic");

        assert_eq!(diagnostic.hint, KillHint::CsaTimeout);
        assert!(!called.get(), "{terminal_reason} should skip memory checks");
        let line = diagnostic
            .stderr_line()
            .expect("timeout signal should render timeout reason");
        assert!(line.contains(terminal_reason));
        assert!(line.contains("concrete kill reason"));
    }
}

#[test]
fn classifies_unknown_signal_without_memory_evidence() {
    let diagnostic = diagnose_signal_kill_with(143, None, KillSignalObservations::default)
        .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    let line = diagnostic
        .stderr_line()
        .expect("unknown signal should render checked evidence");
    assert!(line.contains("unknown_signal"));
    assert!(line.contains("termination_reason: missing"));
    assert!(line.contains("MemAvailable: unavailable"));
    assert!(line.contains("earlyoom not running"));
    assert!(line.contains("reason remains unknown"));
}

#[test]
fn unknown_signal_reports_negative_memory_evidence() {
    let diagnostic = diagnose_signal_kill_with(143, Some("sigterm"), || KillSignalObservations {
        meminfo: Some(MemInfo {
            total_kb: 16_384_000,
            available_kb: 12_288_000,
        }),
        earlyoom_running: false,
        cgroup_memory_events: Some(CgroupMemoryEvents {
            oom: 0,
            oom_kill: 0,
        }),
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    let line = diagnostic
        .stderr_line()
        .expect("unknown signal should render checked evidence");
    assert!(line.contains("termination_reason=sigterm"));
    assert!(line.contains("MemAvailable: 12000 MB / MemTotal: 16000 MB"));
    assert!(line.contains("earlyoom not running"));
    assert!(line.contains("cgroup memory.events oom=0 oom_kill=0"));
    assert!(line.contains("reason remains unknown"));
}

#[test]
fn unknown_signal_last_item_keeps_existing_session_work_item() {
    let diagnostic =
        diagnose_signal_kill_with(143, Some("signal"), KillSignalObservations::default)
            .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    assert!(
        diagnostic.last_item().is_none(),
        "terminal_reason should not replace the session's last known work item"
    );
}

#[test]
fn codex_timeout_wrapped_git_commit_classifies_hook_commit_timeout() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("output.log"),
        concat!(
            r#"{"type":"item.started","item":{"id":"item_64","type":"command_execution","command":"/usr/bin/zsh -lc 'timeout 240s mise exec rust@stable -- git commit -F \"$CSA_SESSION_DIR/commit-message-415.txt\"'","status":"in_progress"}}"#,
            "\n",
            "... process terminated with exit code 143\n",
        ),
    )
    .expect("write output.log");

    let child = detect_child_timeout_provenance(temp.path(), 143)
        .expect("timeout-wrapped git commit should be detected");
    assert_eq!(child.kind, ChildTimeoutKind::HookEnabledGitCommit);
    assert_eq!(child.timeout_seconds, Some(240));
    assert_eq!(child.command_status.as_deref(), Some("in_progress"));
    assert!(child.transcript_exit_143);

    let diagnostic = diagnose_signal_kill_with_child_timeout(
        143,
        None,
        Some(child),
        KillSignalObservations::default,
    )
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::HookCommitTimeout);
    assert!(
        diagnostic
            .last_item()
            .is_some_and(|item| item.contains("git commit -F")),
        "last_item should expose the bounded commit command"
    );
    let line = diagnostic
        .stderr_line()
        .expect("hook commit timeout should render");
    assert!(line.contains("hook_commit_timeout"));
    assert!(line.contains("child_timeout_seconds=240"));
    assert!(line.contains("bounded hook-enabled git commit"));
    assert!(!line.contains("reason remains unknown"));
}

#[test]
fn timeout_wrapped_non_commit_classifies_child_timeout() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("output.log"),
        r#"{"type":"item.started","item":{"type":"command_execution","command":"timeout --foreground 4m cargo test -j1 design_insight","status":"in_progress"}}"#,
    )
    .expect("write output.log");

    let child = detect_child_timeout_provenance(temp.path(), 143)
        .expect("timeout-wrapped command should be detected");
    assert_eq!(child.kind, ChildTimeoutKind::BoundedCommand);
    assert_eq!(child.timeout_seconds, Some(240));

    let diagnostic = diagnose_signal_kill_with_child_timeout(
        143,
        Some("sigterm"),
        Some(child),
        KillSignalObservations::default,
    )
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::ChildTimeout);
    assert!(
        diagnostic
            .stderr_line()
            .expect("child timeout should render")
            .contains("child_timeout")
    );
}

#[test]
fn timeout_child_command_redacts_secrets_from_diagnostic_surfaces() {
    const API_SECRET: &str = "prod-secret";
    const BASIC_AUTH_SECRET: &str = "alice:s3cr3t";
    const SHORT_AUTH_SECRET: &str = "bob:p4ss";
    const PROXY_AUTH_SECRET: &str = "carol:p4ss";
    const INLINE_AUTH_SECRET: &str = "dave:inline";
    const INLINE_PROXY_SECRET: &str = "erin:proxy";
    const SHORT_EQUALS_AUTH_SECRET: &str = "frank:eq";
    const INLINE_API_SECRET: &str = "inline-api-value";
    const INLINE_TOKEN_SECRET: &str = "inline-token-value";
    const INLINE_PASSWORD_SECRET: &str = "inline-password-value";

    let raw_command = format!(
        "timeout 120s curl --user {BASIC_AUTH_SECRET} -u {SHORT_AUTH_SECRET} -u={SHORT_EQUALS_AUTH_SECRET} --proxy-user {PROXY_AUTH_SECRET} --user={INLINE_AUTH_SECRET} --proxy-user={INLINE_PROXY_SECRET} --api-key {API_SECRET} --api-key={INLINE_API_SECRET} --token {API_SECRET} --token={INLINE_TOKEN_SECRET} --password {API_SECRET} --password={INLINE_PASSWORD_SECRET} -H 'x-api-key: {API_SECRET}' --header 'x-api-key: {API_SECRET}' https://example.test"
    );
    assert_timeout_child_command_redacts_secrets_from_all_surfaces(
        &raw_command,
        &[
            API_SECRET,
            BASIC_AUTH_SECRET,
            SHORT_AUTH_SECRET,
            PROXY_AUTH_SECRET,
            INLINE_AUTH_SECRET,
            INLINE_PROXY_SECRET,
            SHORT_EQUALS_AUTH_SECRET,
            INLINE_API_SECRET,
            INLINE_TOKEN_SECRET,
            INLINE_PASSWORD_SECRET,
        ],
        "timeout command",
    );
}

#[test]
fn timeout_child_command_redacts_attached_curl_short_option_secrets_from_diagnostic_surfaces() {
    const BASIC_AUTH_SECRET: &str = "alice:s3cr3t";
    const API_SECRET: &str = "prod-secret";

    let raw_command = format!(
        "timeout 120s curl -u{BASIC_AUTH_SECRET} -Hx-api-key:{API_SECRET} -HAuthorization:Basic {BASIC_AUTH_SECRET} https://example.test"
    );
    assert_timeout_child_command_redacts_secrets_from_all_surfaces(
        &raw_command,
        &[BASIC_AUTH_SECRET, API_SECRET],
        "attached curl short-option",
    );
}

#[test]
fn timeout_child_command_redacts_authorization_basic_header_variants_from_diagnostic_surfaces() {
    const SHORT_COMPACT_SECRET: &str = "alice:s3cr3t-a";
    const SHORT_SPACED_SECRET: &str = "bruce:p4ss-b";
    const LONG_COMPACT_SECRET: &str = "carol:p4ss-c";
    const LONG_SPACED_SECRET: &str = "dave:p4ss-d";
    const SINGLE_QUOTED_SECRET: &str = "erin:p4ss-e";
    const DOUBLE_QUOTED_SECRET: &str = "frank:p4ss-f";
    const EQUALS_COMPACT_SECRET: &str = "grace:p4ss-g";
    const EQUALS_SPACED_SECRET: &str = "heidi:p4ss-h";

    let raw_command = format!(
        "timeout 120s curl -H Authorization:Basic {SHORT_COMPACT_SECRET} -H Authorization: Basic {SHORT_SPACED_SECRET} --header Authorization:Basic {LONG_COMPACT_SECRET} --header Authorization: Basic {LONG_SPACED_SECRET} --header='Authorization: Basic {SINGLE_QUOTED_SECRET}' --header=\"Authorization: Basic {DOUBLE_QUOTED_SECRET}\" --header=Authorization:Basic {EQUALS_COMPACT_SECRET} --header=Authorization: Basic {EQUALS_SPACED_SECRET} https://example.test"
    );

    assert_timeout_child_command_redacts_secrets_from_all_surfaces(
        &raw_command,
        &[
            SHORT_COMPACT_SECRET,
            SHORT_SPACED_SECRET,
            LONG_COMPACT_SECRET,
            LONG_SPACED_SECRET,
            SINGLE_QUOTED_SECRET,
            DOUBLE_QUOTED_SECRET,
            EQUALS_COMPACT_SECRET,
            EQUALS_SPACED_SECRET,
        ],
        "authorization basic header variant",
    );
}

fn assert_timeout_child_command_redacts_secrets_from_all_surfaces(
    raw_command: &str,
    secrets: &[&str],
    context: &str,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let event = serde_json::json!({
        "type": "item.started",
        "item": {
            "type": "command_execution",
            "command": raw_command,
            "status": "in_progress",
        }
    });
    fs::write(
        temp.path().join("output.log"),
        format!("{event}\nprocess terminated with exit code 143\n"),
    )
    .expect("write output.log");

    let child = detect_child_timeout_provenance(temp.path(), 143)
        .expect("timeout-wrapped command should be detected");
    assert_eq!(child.kind, ChildTimeoutKind::BoundedCommand);
    assert_eq!(child.timeout_seconds, Some(120));
    assert!(child.command.contains("[REDACTED]"));
    assert!(child.command.contains("https://example.test"));

    let diagnostic = diagnose_signal_kill_with_child_timeout(
        143,
        Some("sigterm"),
        Some(child.clone()),
        KillSignalObservations::default,
    )
    .expect("signal exit should produce diagnostic");
    assert_eq!(diagnostic.hint, KillHint::ChildTimeout);

    let stderr_line = diagnostic
        .stderr_line()
        .expect("child timeout should render");
    let last_item = diagnostic
        .last_item()
        .expect("child timeout should expose redacted last item");
    let now = chrono::Utc::now();
    let result = csa_session::SessionResult {
        status: "signal".to_string(),
        exit_code: 143,
        summary: "Execution interrupted by SIGTERM".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        ..Default::default()
    };
    let rendered =
        render_result_toml_with_signal_diagnostic(&result, Some(&diagnostic), Some(raw_command))
            .expect("render signal result");

    for (label, surface) in [
        ("detected command", child.command.as_str()),
        ("stderr line", stderr_line.as_str()),
        ("last item", last_item.as_str()),
        ("result toml", rendered.as_str()),
    ] {
        for secret in secrets {
            assert!(
                !surface.contains(secret),
                "{label} should redact {context} secret {secret}: {surface}"
            );
        }
    }
    assert!(stderr_line.contains("[REDACTED]"));
    assert!(last_item.contains("[REDACTED]"));
    assert!(rendered.contains("[REDACTED]"));
    assert!(rendered.contains("kill_hint = \"child_timeout\""));
    assert!(rendered.contains("last_item ="));
}

#[test]
fn completed_timeout_command_without_child_exit_143_stays_unknown() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("output.log"),
        concat!(
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"timeout 240s cargo test -j1 design_insight","exit_code":0,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
        ),
    )
    .expect("write output.log");

    assert!(
        detect_child_timeout_provenance(temp.path(), 143).is_none(),
        "a previously completed bounded command is not enough evidence for child timeout"
    );
}

#[test]
fn external_sigterm_without_timeout_child_evidence_stays_unknown() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(
        temp.path().join("output.log"),
        r#"{"type":"item.started","item":{"type":"command_execution","command":"git status","status":"in_progress"}}"#,
    )
    .expect("write output.log");

    assert!(detect_child_timeout_provenance(temp.path(), 143).is_none());

    let diagnostic = diagnose_signal_kill_with_child_timeout(
        143,
        Some("sigterm"),
        None,
        KillSignalObservations::default,
    )
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
}

fn memory_soft_limit_event() -> csa_resource::memory_monitor::MemorySoftLimitKillDiagnostic {
    csa_resource::memory_monitor::MemorySoftLimitKillDiagnostic {
        kill_hint: csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 9216,
        threshold_mb: 8601,
        memory_max_mb: 12_288,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01KTEST.scope".to_string(),
    }
}

fn test_session_dir(temp: &tempfile::TempDir, session_id: &str) -> PathBuf {
    let session_dir = temp.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    session_dir
}

fn register_memory_soft_limit_evidence(session_dir: &Path) {
    let registry_key =
        csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(session_dir)
            .expect("CSA-owned memory diagnostic path");
    assert!(
        !registry_key.starts_with(session_dir),
        "memory-soft-limit evidence must live outside child-writable session dir"
    );
    let event = memory_soft_limit_event();
    csa_resource::memory_monitor::record_soft_limit_diagnostic_evidence(&registry_key, &event);
}

#[test]
fn csa_memory_soft_limit_monitor_evidence_classifies_sigterm_concretely() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01KTESTMEMCLASSIFY";
    let session_dir = test_session_dir(&temp, session_id);
    register_memory_soft_limit_evidence(&session_dir);

    let diagnostic =
        diagnose_signal_kill(143, Some("signal"), "codex", session_id, Some(&session_dir))
            .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::MemorySoftLimit);
    assert_eq!(
        diagnostic
            .result_report()
            .as_ref()
            .map(|report| report.source.as_str()),
        Some("memory_soft_limit")
    );
    let line = diagnostic
        .stderr_line()
        .expect("memory soft limit should render");
    assert!(line.contains("memory soft limit"));
    assert!(line.contains("current_mb=9216"));
    assert!(line.contains("threshold_mb=8601"));
    assert!(line.contains("memory_max_mb=12288"));
    assert!(line.contains("soft_limit_percent=70"));
    assert!(line.contains("scope_name=csa-codex-01KTEST.scope"));
    assert!(!line.contains("unknown_signal"));
}

#[test]
fn forged_child_writable_memory_soft_limit_artifact_stays_unknown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = test_session_dir(&temp, "01KTESTFORGEDOUTPUT");
    let forged_path = session_dir.join("output/memory-soft-limit-kill.toml");
    std::fs::create_dir_all(forged_path.parent().expect("forged artifact parent"))
        .expect("create child-writable output dir");
    std::fs::write(
        &forged_path,
        toml::to_string_pretty(&memory_soft_limit_event()).expect("serialize event"),
    )
    .expect("write forged output artifact");

    let past_start = chrono::Utc::now() - chrono::Duration::seconds(60);
    let memory_soft_limit = read_memory_soft_limit_diagnostic(&session_dir, Some(&past_start));
    assert!(
        memory_soft_limit.is_none(),
        "child-writable output artifact must not be authoritative CSA evidence"
    );

    let diagnostic = diagnose_signal_kill_with_events(
        143,
        Some("signal"),
        memory_soft_limit,
        None,
        None,
        KillSignalObservations::default,
    )
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::UnknownSignal);
    assert!(diagnostic.result_report().is_none());
}

#[test]
fn stale_memory_soft_limit_registry_evidence_is_ignored_for_result_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = test_session_dir(&temp, "01KTESTMEMWINDOW");
    register_memory_soft_limit_evidence(&session_dir);

    let future_start = chrono::Utc::now() + chrono::Duration::seconds(60);
    let past_start = chrono::Utc::now() - chrono::Duration::seconds(60);

    assert!(read_memory_soft_limit_diagnostic(&session_dir, Some(&future_start)).is_none());
    assert!(read_memory_soft_limit_diagnostic(&session_dir, Some(&past_start)).is_some());
}

#[test]
fn memory_pressure_precedes_child_timeout_evidence() {
    let child = ChildTimeoutProvenance {
        command: "timeout 240s git commit -m test".to_string(),
        timeout_seconds: Some(240),
        kind: ChildTimeoutKind::HookEnabledGitCommit,
        command_status: Some("in_progress".to_string()),
        transcript_exit_143: true,
    };

    let diagnostic = diagnose_signal_kill_with_child_timeout(143, None, Some(child), || {
        KillSignalObservations {
            meminfo: Some(MemInfo {
                total_kb: 10_000,
                available_kb: 499,
            }),
            earlyoom_running: false,
            cgroup_memory_events: None,
        }
    })
    .expect("signal exit should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::MemoryPressure);
    assert!(diagnostic.child_timeout.is_none());
}

#[test]
fn non_signal_exits_do_not_collect_observations() {
    for exit_code in [0, 1, 2] {
        let called = Cell::new(false);
        let diagnostic = diagnose_signal_kill_with(exit_code, None, || {
            called.set(true);
            KillSignalObservations::default()
        });
        assert!(diagnostic.is_none());
        assert!(!called.get(), "exit {exit_code} should not trigger checks");
    }
}
