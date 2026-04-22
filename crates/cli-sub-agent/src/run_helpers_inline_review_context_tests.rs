use super::prepend_review_context_to_prompt;
use crate::cli::{Cli, Commands};
use crate::test_session_sandbox::ScopedSessionSandbox;
use clap::Parser;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct SharedLogBuffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.bytes.lock().expect("lock log buffer").clone())
            .expect("buffer should contain valid utf-8")
    }
}

struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("lock log writer")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn create_review_session(project_root: &Path, session_id: &str) -> PathBuf {
    let session_dir = csa_session::get_session_dir(project_root, session_id).expect("session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    session_dir
}

#[test]
fn prepend_review_context_to_prompt_includes_all_review_artifacts() {
    let project_dir = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8Q9";
    let session_dir = create_review_session(project_dir.path(), session_id);
    fs::write(
        session_dir.join("output").join("summary.md"),
        "Summary line\n",
    )
    .unwrap();
    fs::write(
        session_dir.join("output").join("details.md"),
        "Details line\n",
    )
    .unwrap();
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .unwrap();

    let prompt = prepend_review_context_to_prompt(
        project_dir.path(),
        "Fix the bug".to_string(),
        Some(session_id),
    )
    .expect("prompt should render");

    let expected = concat!(
        "<csa-review-context session=\"01KAS6M5XG7V4M4M6YDRS7P8Q9\">\n",
        "<!-- summary.md -->\n",
        "Summary line\n",
        "<!-- details.md -->\n",
        "Details line\n",
        "<!-- findings.toml -->\n",
        "findings = []\n",
        "</csa-review-context>\n\n",
        "<original-prompt>\n",
        "Fix the bug\n",
        "</original-prompt>\n"
    );
    assert_eq!(prompt, expected);
}

#[test]
fn prepend_review_context_to_prompt_skips_missing_artifacts() {
    let project_dir = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R0";
    let session_dir = create_review_session(project_dir.path(), session_id);
    fs::write(
        session_dir.join("output").join("summary.md"),
        "Only summary\n",
    )
    .unwrap();

    let prompt = prepend_review_context_to_prompt(
        project_dir.path(),
        "Fix the bug".to_string(),
        Some(session_id),
    )
    .expect("prompt should render");

    assert!(prompt.contains("<!-- summary.md -->\nOnly summary\n"));
    assert!(!prompt.contains("<!-- details.md -->"));
    assert!(!prompt.contains("<!-- findings.toml -->"));
    assert!(prompt.ends_with("<original-prompt>\nFix the bug\n</original-prompt>\n"));
}

#[test]
fn prepend_review_context_to_prompt_warns_and_leaves_prompt_unchanged_when_outputs_missing() {
    let project_dir = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R1";
    let _session_dir = create_review_session(project_dir.path(), session_id);
    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(buffer.clone())
        .without_time()
        .finish();

    let prompt = tracing::subscriber::with_default(subscriber, || {
        prepend_review_context_to_prompt(
            project_dir.path(),
            "Fix the bug".to_string(),
            Some(session_id),
        )
    })
    .expect("prompt should remain unchanged");

    assert_eq!(prompt, "Fix the bug");
    assert!(buffer.contents().contains(
        "Inline review context requested but summary/details/findings artifacts were missing"
    ));
}

#[test]
fn prepend_review_context_to_prompt_errors_for_unknown_session() {
    let project_dir = tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let err = prepend_review_context_to_prompt(
        project_dir.path(),
        "Fix the bug".to_string(),
        Some("01KAS6M5XG7V4M4M6YDRS7P8R2"),
    )
    .expect_err("missing session should fail");

    assert_eq!(
        err.to_string(),
        "--inline-context-from-review-session: session 01KAS6M5XG7V4M4M6YDRS7P8R2 not found"
    );
}

#[test]
fn cli_run_inline_context_from_review_session_accepts_ulid() {
    let cli = Cli::try_parse_from([
        "csa",
        "run",
        "--inline-context-from-review-session",
        "01KAS6M5XG7V4M4M6YDRS7P8R3",
        "fix it",
    ])
    .expect("cli should parse");

    match cli.command {
        Commands::Run {
            inline_context_from_review_session,
            ..
        } => {
            assert_eq!(
                inline_context_from_review_session.as_deref(),
                Some("01KAS6M5XG7V4M4M6YDRS7P8R3")
            );
        }
        _ => panic!("expected run command"),
    }
}

#[test]
fn cli_run_inline_context_from_review_session_rejects_non_ulid() {
    let err = match Cli::try_parse_from([
        "csa",
        "run",
        "--inline-context-from-review-session",
        "not-a-ulid",
        "fix it",
    ]) {
        Ok(_) => panic!("non-ulid should be rejected"),
        Err(err) => err,
    };

    let rendered = err.to_string();
    assert!(rendered.contains("--inline-context-from-review-session"));
    assert!(rendered.contains("Invalid session ID"));
}
