//! Tests for prompt transport selection between argv and stdin.

use super::*;

/// Helper: create a minimal MetaSessionState for testing.
fn make_test_session() -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test session".to_string()),
        project_path: "/tmp/test-project".to_string(),
        created_at: now,
        last_accessed: now,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
    }
}

#[test]
fn test_build_command_short_prompt_uses_argv_and_no_stdin_data() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        suppress_notify: false,
    };
    let session = make_test_session();
    let prompt = "short prompt";

    let (cmd, stdin_data) = exec.build_command(prompt, None, &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(stdin_data.is_none(), "short prompts should not use stdin");
    assert!(
        args.contains(&prompt.to_string()),
        "prompt should stay in argv"
    );
}

#[test]
fn test_build_command_long_prompt_uses_stdin_for_stdin_capable_tool() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        suppress_notify: false,
    };
    let session = make_test_session();
    let prompt = "p".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let (cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&prompt),
        "long prompt should not be present in argv when stdin transport is selected"
    );
    assert_eq!(
        stdin_data,
        Some(prompt.as_bytes().to_vec()),
        "stdin payload should carry the full prompt bytes"
    );
}

#[test]
fn test_build_command_long_prompt_uses_stdin_for_gemini_cli() {
    let exec = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let prompt = "g".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let (cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&prompt),
        "long prompt should not be in argv for gemini-cli"
    );
    assert_eq!(
        stdin_data,
        Some(prompt.as_bytes().to_vec()),
        "gemini-cli should transport long prompts via stdin"
    );
}

#[test]
fn test_build_command_long_prompt_uses_stdin_for_claude_code() {
    let exec = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let prompt = "c".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let (cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        !args.contains(&prompt),
        "long prompt should not be in argv for claude-code"
    );
    assert_eq!(
        stdin_data,
        Some(prompt.as_bytes().to_vec()),
        "claude-code should transport long prompts via stdin"
    );
}

#[test]
fn test_build_command_long_prompt_opencode_stays_in_argv() {
    let exec = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let prompt = "o".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let (cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert!(
        stdin_data.is_none(),
        "argv-only tools should not return stdin payload"
    );
    assert!(
        args.contains(&prompt),
        "argv-only tools must keep prompt in argv even when long"
    );
}

#[test]
fn test_build_command_long_prompt_opencode_emits_warning() {
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct SharedBufferWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl io::Write for SharedBufferWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut guard = self.buf.lock().expect("buffer lock poisoned");
            guard.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct SharedMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl<'a> MakeWriter<'a> for SharedMakeWriter {
        type Writer = SharedBufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufferWriter {
                buf: Arc::clone(&self.buf),
            }
        }
    }

    let exec = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };
    let session = make_test_session();
    let prompt = "w".repeat(MAX_ARGV_PROMPT_LEN + 1);

    let log_buf = Arc::new(Mutex::new(Vec::new()));
    let make_writer = SharedMakeWriter {
        buf: Arc::clone(&log_buf),
    };
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_writer(make_writer)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        let (_cmd, stdin_data) = exec.build_command(&prompt, None, &session, None);
        assert!(
            stdin_data.is_none(),
            "argv-only tools should not return stdin payload"
        );
    });

    let logs = String::from_utf8(log_buf.lock().expect("buffer lock poisoned").clone())
        .expect("logs should be valid UTF-8");
    assert!(
        logs.contains("Prompt exceeds argv threshold; tool supports argv-only transport"),
        "Expected warning log, got: {logs}"
    );
}
