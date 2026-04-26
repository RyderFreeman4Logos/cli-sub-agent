//! Tests for `plan_cmd_daemon`.

use super::*;
use crate::plan_cmd::PlanRunArgs;

fn make_args() -> PlanRunArgs {
    PlanRunArgs {
        file: Some("workflow.toml".to_string()),
        pattern: None,
        vars: vec![],
        tool_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        cd: None,
        current_depth: 0,
    }
}

#[test]
fn describe_uses_pattern_name_when_set() {
    let mut args = make_args();
    args.file = None;
    args.pattern = Some("dev2merge".to_string());
    assert_eq!(describe_plan_run(&args), "plan: dev2merge");
}

#[test]
fn describe_falls_back_to_file_path() {
    let args = make_args();
    assert_eq!(describe_plan_run(&args), "plan: workflow.toml");
}

#[test]
fn describe_handles_resume_form() {
    let mut args = make_args();
    args.file = None;
    args.resume = Some("/tmp/journal.json".to_string());
    assert_eq!(describe_plan_run(&args), "plan: --resume /tmp/journal.json");
}

#[test]
fn describe_unknown_when_no_source_provided() {
    let mut args = make_args();
    args.file = None;
    assert_eq!(describe_plan_run(&args), "plan: (unknown workflow)");
}

#[test]
fn forwarded_args_strip_through_plan_run() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "patterns/dev2merge/workflow.toml".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--var".to_string(),
        "FEATURE_INPUT=test".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(
        forwarded,
        vec![
            "patterns/dev2merge/workflow.toml",
            "--sa-mode",
            "true",
            "--var",
            "FEATURE_INPUT=test",
        ]
    );
}

#[test]
fn forwarded_args_drop_foreground_flag() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--foreground".to_string(),
        "workflow.toml".to_string(),
    ];
    // The `--foreground` flag is the parent-only opt-out and must not be
    // forwarded to the daemon child (which IS the worker, not a re-spawn).
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["workflow.toml"]);
}

#[test]
fn forwarded_args_handle_global_flags_before_plan() {
    let argv = vec![
        "csa".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--pattern".to_string(),
        "dev2merge".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["--pattern", "dev2merge"]);
}

#[test]
fn forwarded_args_empty_when_plan_missing() {
    let argv = vec!["csa".to_string(), "run".to_string()];
    assert!(build_forwarded_plan_args(&argv).is_empty());
}

#[test]
fn forwarded_args_preserve_foreground_after_double_dash() {
    // F3: stripping is `--`-aware. A literal `--foreground` AFTER a `--`
    // positional separator is a workflow argument, not the parent-only opt-out
    // flag, and must be forwarded intact.
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--foreground".to_string(),
        "workflow.toml".to_string(),
        "--".to_string(),
        "--foreground".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(
        forwarded,
        vec!["workflow.toml", "--", "--foreground"],
        "literal --foreground after `--` must be preserved as a positional"
    );
}

// ---------------------------------------------------------------------------
// F1 — depth-aware daemon flip gating
// ---------------------------------------------------------------------------

fn base_input() -> ForegroundDecisionInput {
    ForegroundDecisionInput {
        foreground: false,
        dry_run: false,
        chunked: false,
        has_resume: false,
        current_depth: 0,
        nested_env: false,
    }
}

#[test]
fn dispatch_with_no_session_env_and_depth_zero_takes_daemon_path() {
    // Top-level user invocation: clean env, depth=0, no opt-out flags →
    // daemonize (needs_foreground=false).
    assert!(!decide_needs_foreground(base_input()));
}

#[test]
fn dispatch_with_csa_depth_gt_zero_forces_foreground() {
    // Nested via bash-step CSA_DEPTH bump (plan_cmd_exec::spawn_bash sets
    // CSA_DEPTH = parent_depth + 1 for every workflow bash step).
    let mut input = base_input();
    input.current_depth = 1;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_with_csa_session_id_in_env_forces_foreground() {
    // Nested via inherited session-marker env (handle_plan_run_daemon_child
    // sets CSA_SESSION_ID; bash steps inherit it; the nested csa plan run
    // invocation reads it back).
    let mut input = base_input();
    input.nested_env = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_explicit_foreground_still_runs_inline_at_depth_zero() {
    // User opt-out at root depth — the existing --foreground escape hatch
    // must keep working independently of nested-invocation detection.
    let mut input = base_input();
    input.foreground = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_dry_run_forces_foreground_at_depth_zero() {
    // --dry-run prints the plan synchronously; daemonizing would lose the
    // stdout output the user is asking for.
    let mut input = base_input();
    input.dry_run = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_chunked_forces_foreground_at_depth_zero() {
    let mut input = base_input();
    input.chunked = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_resume_forces_foreground_at_depth_zero() {
    let mut input = base_input();
    input.has_resume = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_depth_gt_zero_overrides_all_opt_outs() {
    // Even if a future caller tries to "force daemon" by passing a clean
    // input shape, depth>0 is sticky — nested daemonization is never safe.
    let input = ForegroundDecisionInput {
        foreground: false,
        dry_run: false,
        chunked: false,
        has_resume: false,
        current_depth: 3,
        nested_env: false,
    };
    assert!(decide_needs_foreground(input));
}

// ---------------------------------------------------------------------------
// F1 — env-marker probe
// ---------------------------------------------------------------------------
//
// The env-mutating tests below use serial_test to avoid races with other
// CSA_*_SESSION_ID readers in the test binary. Each test snapshots the
// markers it touches and restores them on exit.

mod env_probe {
    use super::*;
    use serial_test::serial;

    const MARKERS: &[&str] = &[
        "CSA_SESSION_ID",
        "CSA_DAEMON_SESSION_ID",
        "CSA_PARENT_SESSION_ID",
    ];

    fn snapshot_markers() -> Vec<(&'static str, Option<String>)> {
        MARKERS
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect()
    }

    /// Restore previously-snapshotted env markers. Called at the end of
    /// every test in this module via the [`EnvGuard`] RAII wrapper.
    fn restore_markers(snapshot: Vec<(&'static str, Option<String>)>) {
        for (key, value) in snapshot {
            // SAFETY: test-only; runs after all reads and before module
            // teardown. serial_test ensures no concurrent access.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    struct EnvGuard {
        snapshot: Option<Vec<(&'static str, Option<String>)>>,
    }
    impl EnvGuard {
        fn capture_and_clear() -> Self {
            let snapshot = snapshot_markers();
            for key in MARKERS {
                // SAFETY: test-only; serial_test ensures no concurrent access.
                unsafe { std::env::remove_var(key) };
            }
            Self {
                snapshot: Some(snapshot),
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(snap) = self.snapshot.take() {
                restore_markers(snap);
            }
        }
    }

    fn set_marker(key: &str, value: &str) {
        // SAFETY: test-only; serial_test ensures no concurrent access.
        unsafe { std::env::set_var(key, value) };
    }

    #[test]
    #[serial]
    fn nested_env_clean_returns_false() {
        let _guard = EnvGuard::capture_and_clear();
        assert!(!nested_session_env_present());
    }

    #[test]
    #[serial]
    fn nested_env_with_csa_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(nested_session_env_present());
    }

    #[test]
    #[serial]
    fn nested_env_with_csa_daemon_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_DAEMON_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(nested_session_env_present());
    }

    #[test]
    #[serial]
    fn nested_env_with_csa_parent_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_PARENT_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(nested_session_env_present());
    }

    #[test]
    #[serial]
    fn nested_env_with_empty_marker_returns_false() {
        // Edge case: empty string is treated as "not set" so callers that
        // accidentally exported an empty value don't trigger the gate.
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_SESSION_ID", "");
        assert!(!nested_session_env_present());
    }
}
