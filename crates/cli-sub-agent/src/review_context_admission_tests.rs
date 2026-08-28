use crate::cli::{Cli, Commands, validate_review_args};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use clap::Parser;
use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument};
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::time::Duration;
use tempfile::tempdir;

use super::*;

fn sample_spec_document(plan_ulid: &str, criterion_id: &str) -> SpecDocument {
    SpecDocument {
        schema_version: 1,
        plan_ulid: plan_ulid.to_string(),
        summary: format!("Spec summary for {plan_ulid}"),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Scenario,
            id: criterion_id.to_string(),
            description: format!("Criterion {criterion_id} must be satisfied."),
            status: CriterionStatus::Pending,
        }],
    }
}

#[test]
fn daemon_review_context_snapshot_survives_source_replacement() {
    let project = tempdir().unwrap();
    let session = tempdir().unwrap();
    let path = project.path().join("TODO.md");
    std::fs::write(&path, "admitted context").unwrap();
    let parent = resolve_review_context(Some("TODO.md"), project.path(), false)
        .unwrap()
        .unwrap();

    parent.persist_daemon_snapshot(session.path()).unwrap();
    std::fs::write(&path, "replacement context").unwrap();
    let lock = TEST_ENV_LOCK.clone();
    let _lock = lock.blocking_lock();
    let _launch_digest =
        ScopedEnvVarRestore::set(DAEMON_REVIEW_CONTEXT_DIGEST_ENV_KEY, &parent.digest);
    let child = ResolvedReviewContext::load_daemon_snapshot(session.path())
        .unwrap()
        .expect("daemon child should receive the admitted snapshot");

    assert_eq!(child.snapshot(), "admitted context");
    assert_eq!(child.digest, parent.digest);
    assert_eq!(child.kind, parent.kind);
}

#[test]
fn daemon_child_rejects_replacement_with_recomputed_digest_for_each_context_flag() {
    let lock = TEST_ENV_LOCK.clone();
    let _lock = lock.blocking_lock();
    let project = tempdir().unwrap();
    let session_id = "daemon-context-digest-binding";
    let session_dir = csa_session::get_session_dir(project.path(), session_id).unwrap();

    for (flag, file_name, admitted, replacement, kind) in [
        (
            "--spec",
            "review.toml",
            "plan_ulid = \"01JTESTPLAN0000000000000003\"\nsummary = \"admitted spec\"\n",
            "plan_ulid = \"01JTESTPLAN0000000000000004\"\nsummary = \"replacement spec\"\n",
            "SpecToml",
        ),
        (
            "--context",
            "context.md",
            "admitted context",
            "replacement context",
            "TodoMarkdown",
        ),
        (
            "--prompt-file",
            "prompt.md",
            "admitted prompt",
            "replacement prompt",
            "TodoMarkdown",
        ),
    ] {
        let path = project.path().join(file_name);
        std::fs::write(&path, admitted).unwrap();
        let parent = parse_review_args(project.path(), &[flag, path.to_str().unwrap()]);
        let admitted_context =
            resolve_review_context_for_args(&parent, project.path(), false, None)
                .unwrap()
                .expect("parent should admit explicit context");
        let _launch_digest = ScopedEnvVarRestore::set(
            DAEMON_REVIEW_CONTEXT_DIGEST_ENV_KEY,
            &admitted_context.digest,
        );
        admitted_context
            .persist_daemon_snapshot(&session_dir)
            .unwrap();

        let replacement_kind = match kind {
            "SpecToml" => "spec-toml",
            "TodoMarkdown" => "todo-markdown",
            "Passthrough" => "passthrough",
            _ => unreachable!("serialized daemon context kind"),
        };
        let replacement_digest = digest_review_context(replacement_kind, replacement);
        let replacement_snapshot = serde_json::json!({
            "digest": replacement_digest,
            "kind": kind,
            "snapshot": replacement,
        });
        std::fs::write(
            session_dir.join("input/review-context.json"),
            serde_json::to_vec(&replacement_snapshot).unwrap(),
        )
        .unwrap();

        let child = parse_review_args(
            project.path(),
            &[
                "--daemon-child",
                "--session-id",
                session_id,
                flag,
                path.to_str().unwrap(),
            ],
        );
        let error = resolve_review_context_for_args(&child, project.path(), false, None)
            .expect_err("daemon child must reject a replacement snapshot");
        assert!(format!("{error:#}").contains("admitted daemon review context digest mismatch"));
    }
}

#[test]
fn daemon_child_rejects_kind_tampering_for_each_context_flag() {
    let lock = TEST_ENV_LOCK.clone();
    let _lock = lock.blocking_lock();
    let project = tempdir().unwrap();
    let session_id = "daemon-context-kind-binding";
    let session_dir = csa_session::get_session_dir(project.path(), session_id).unwrap();

    for (flag, file_name, admitted) in [
        (
            "--spec",
            "review.toml",
            r#"plan_ulid = "01JTESTPLAN0000000000000003"
summary = "admitted spec"
"#,
        ),
        ("--context", "context.md", "admitted context"),
        ("--prompt-file", "prompt.md", "admitted prompt"),
    ] {
        let path = project.path().join(file_name);
        std::fs::write(&path, admitted).unwrap();
        let parent = parse_review_args(project.path(), &[flag, path.to_str().unwrap()]);
        let admitted_context =
            resolve_review_context_for_args(&parent, project.path(), false, None)
                .unwrap()
                .expect("parent should admit explicit context");
        let _launch_digest = ScopedEnvVarRestore::set(
            DAEMON_REVIEW_CONTEXT_DIGEST_ENV_KEY,
            &admitted_context.digest,
        );
        admitted_context
            .persist_daemon_snapshot(&session_dir)
            .unwrap();

        let snapshot_path = session_dir.join("input/review-context.json");
        let mut tampered: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        tampered["kind"] = serde_json::Value::String("Passthrough".to_string());
        std::fs::write(&snapshot_path, serde_json::to_vec(&tampered).unwrap()).unwrap();

        let child = parse_review_args(
            project.path(),
            &[
                "--daemon-child",
                "--session-id",
                session_id,
                flag,
                path.to_str().unwrap(),
            ],
        );
        let error = resolve_review_context_for_args(&child, project.path(), false, None)
            .expect_err("daemon child must reject kind tampering");
        assert!(format!("{error:#}").contains("admitted daemon review context digest mismatch"));
    }
}

#[cfg(unix)]
#[test]
fn daemon_snapshot_fifo_fails_without_blocking() {
    let session = tempdir().unwrap();
    let input_dir = session.path().join("input");
    std::fs::create_dir(&input_dir).unwrap();
    let fifo = input_dir.join("review-context.json");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `fifo_name` is NUL-free and remains alive for the call; the
    // assertion checks `mkfifo`'s return status before the FIFO is used.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

    let (sender, receiver) = std::sync::mpsc::channel();
    let session_path = session.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        sender
            .send(ResolvedReviewContext::load_daemon_snapshot(&session_path))
            .unwrap();
    });
    let finished = match receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(result) => {
            let error = result.expect_err("FIFO snapshot must be rejected");
            assert!(format!("{error:#}").contains("regular"));
            true
        }
        Err(_) => {
            let _writer = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap();
            let _ = receiver.recv_timeout(Duration::from_secs(1));
            false
        }
    };
    worker.join().unwrap();
    assert!(finished, "FIFO snapshot open exceeded the bounded deadline");
}

#[test]
fn daemon_snapshot_round_trips_max_escaped_context_without_source_path() {
    let session = tempdir().unwrap();
    let snapshot = "\0".repeat(REVIEW_CONTEXT_MAX_BYTES);
    let parent = ResolvedReviewContext {
        path: "x".repeat(16 * 1024),
        digest: digest_review_context("todo-markdown", &snapshot),
        kind: ResolvedReviewContextKind::TodoMarkdown,
        snapshot,
    };

    parent.persist_daemon_snapshot(session.path()).unwrap();
    let lock = TEST_ENV_LOCK.clone();
    let _lock = lock.blocking_lock();
    let _launch_digest =
        ScopedEnvVarRestore::set(DAEMON_REVIEW_CONTEXT_DIGEST_ENV_KEY, &parent.digest);
    let child = ResolvedReviewContext::load_daemon_snapshot(session.path())
        .unwrap()
        .expect("daemon child should accept the maximum admitted context");

    assert_eq!(child.snapshot(), parent.snapshot());
    assert_eq!(child.digest, parent.digest);
    assert!(child.path.is_empty());
}

#[test]
fn daemon_child_rejects_missing_admitted_snapshot_for_explicit_context() {
    let project = tempdir().unwrap();
    let session_id = "daemon-context-snapshot";
    let session_dir = csa_session::get_session_dir(project.path(), session_id).unwrap();

    for (flag, file_name, admitted) in [
        (
            "--spec",
            "review.toml",
            "plan_ulid = \"01JTESTPLAN0000000000000003\"\nsummary = \"admitted spec\"\n",
        ),
        ("--context", "context.md", "admitted context"),
        ("--prompt-file", "prompt.md", "admitted prompt"),
    ] {
        let path = project.path().join(file_name);
        let path_arg = path.to_str().unwrap();
        std::fs::write(&path, admitted).unwrap();
        let parent = parse_review_args(project.path(), &[flag, path_arg]);
        let admitted_context =
            resolve_review_context_for_args(&parent, project.path(), false, None)
                .unwrap()
                .expect("parent should admit explicit context");
        admitted_context
            .persist_daemon_snapshot(&session_dir)
            .unwrap();
        std::fs::remove_file(session_dir.join("input/review-context.json")).unwrap();
        std::fs::write(&path, "replacement context").unwrap();

        let child = parse_review_args(
            project.path(),
            &["--daemon-child", "--session-id", session_id, flag, path_arg],
        );
        let error = resolve_review_context_for_args(&child, project.path(), false, None)
            .expect_err("daemon child must reject a missing admitted snapshot");

        assert!(format!("{error:#}").contains("missing admitted daemon review context"));
    }
}

fn parse_review_args(project_root: &std::path::Path, extra: &[&str]) -> crate::cli::ReviewArgs {
    let cd = project_root.display().to_string();
    let mut argv = vec!["csa", "review", "--cd", cd.as_str(), "--diff"];
    argv.extend_from_slice(extra);
    match Cli::try_parse_from(argv).unwrap().command {
        Commands::Review(args) => {
            validate_review_args(&args).unwrap();
            args
        }
        _ => unreachable!(),
    }
}

#[test]
fn resolve_review_context_accepts_dot_relative_explicit_paths() {
    let project = tempdir().unwrap();
    std::fs::write(project.path().join("TODO.md"), "todo context").unwrap();
    std::fs::write(
        project.path().join("spec.toml"),
        toml::to_string_pretty(&sample_spec_document(
            "01JTESTPLAN0000000000000002",
            "criterion-dot-relative",
        ))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(project.path().join("prompt.md"), "prompt context").unwrap();

    for path in ["./TODO.md", "./spec.toml", "./prompt.md"] {
        let context = resolve_review_context(Some(path), project.path(), false)
            .unwrap_or_else(|error| panic!("{path} should be admitted: {error:#}"))
            .expect("explicit path should resolve");
        assert!(!context.snapshot().is_empty(), "{path}");
    }

    let parent = resolve_review_context(Some("../TODO.md"), project.path(), false)
        .expect_err("parent traversal must stay rejected");
    assert!(format!("{parent:#}").contains("must be a file beneath"));
}
