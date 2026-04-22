pub(crate) const ATOMIC_COMMIT_DISCIPLINE_PREAMBLE: &str = r#"<atomic-commit-discipline>
When this task involves multiple independent logical changes (different
files, different concerns, different issue references), finish each
logical change with its own commit before starting the next. Never
batch unrelated logical changes into one commit.

For EACH logical change:
  1. Make the code edits for that change only.
  2. Invoke the `/commit` skill to handle staging, pre-commit gates,
     two-layer review, and conventional-commit message generation.
     Do NOT run manual Git staging, commit, or push commands —
     those are forbidden by AGENTS.md rule 015. The `/commit`
     skill is the only sanctioned commit path.
  3. Verify the working tree is clean (post-skill) before starting
     the next logical change.

Filesystem persistence: when the bwrap filesystem sandbox is active,
writes to host paths like `/tmp/`, `/var/tmp/`, and `$TMPDIR` are NOT
persisted outside this session. Write deliverables the caller must read after session end
to `$CSA_SESSION_DIR/output/<name>.md` — the session output directory IS persisted
and is the canonical location for cross-session artifacts.

If a single logical change must touch multiple unrelated files,
explain the grouping to `/commit` (it surfaces in the commit body).
</atomic-commit-discipline>"#;

const ATOMIC_COMMIT_DISCIPLINE_SUBPROCESS_PREAMBLE: &str = r#"<atomic-commit-discipline>
When this task involves multiple independent logical changes (different
files, different concerns, different issue references), finish each
logical change with its own commit before starting the next. Never
batch unrelated logical changes into one commit.

For EACH logical change:
  1. Make the code edits for that change only.
  2. Stage the specific files for that change: `git add <path> [<path>...]`.
     Avoid `git add -A` (may catch forbidden files per AGENTS.md 036).
  3. Commit with a Conventional Commits message:
     `git commit -m "type(scope): summary" -m "body"`.
  4. Verify the working tree reflects the intended state before
     starting the next logical change.

NOTE: In interactive Claude Code sessions the commit skill handles
staging, pre-commit gates, two-layer review, and message generation.
That skill is NOT available in this CSA subprocess context, so direct
`git add` + `git commit -m` is the sanctioned path here.

Filesystem persistence: when the bwrap filesystem sandbox is active,
writes to host paths like `/tmp/`, `/var/tmp/`, and `$TMPDIR` are NOT
persisted outside this session. Write deliverables the caller must read after session end
to `$CSA_SESSION_DIR/output/<name>.md` — the session output directory IS persisted
and is the canonical location for cross-session artifacts.

If a single logical change must touch multiple unrelated files,
mention the grouping in the commit body.
</atomic-commit-discipline>"#;

fn is_csa_subprocess() -> bool {
    std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|depth| depth > 0)
        .unwrap_or_else(|| {
            std::env::var_os("CSA_SESSION_ID").is_some()
                || std::env::var_os("CSA_DAEMON_SESSION_ID").is_some()
        })
}

pub(crate) fn atomic_commit_discipline_preamble() -> &'static str {
    if is_csa_subprocess() {
        ATOMIC_COMMIT_DISCIPLINE_SUBPROCESS_PREAMBLE
    } else {
        ATOMIC_COMMIT_DISCIPLINE_PREAMBLE
    }
}

pub(crate) fn prepend_atomic_commit_discipline_to_prompt(prompt: String) -> String {
    if prompt.contains("<atomic-commit-discipline>")
        || prompt.starts_with("# REVIEW:")
        || prompt.starts_with("# DEBATE:")
    {
        return prompt;
    }

    format!("{}\n\n{prompt}", atomic_commit_discipline_preamble())
}

#[cfg(test)]
mod tests {
    use super::is_csa_subprocess;
    use crate::test_env_lock::{ScopedEnvVarRestore, ScopedTestEnvVar};
    use csa_core::env::CSA_SESSION_DIR_ENV_KEY;
    use csa_executor::Executor;
    use csa_session::state::MetaSessionState;
    use std::collections::HashMap;
    use std::ffi::OsStr;

    fn make_test_session() -> MetaSessionState {
        let now = chrono::Utc::now();
        MetaSessionState {
            meta_session_id: "01HTEST000000000000000000".to_string(),
            project_path: "/tmp/test-project".to_string(),
            created_at: now,
            last_accessed: now,
            ..MetaSessionState::default()
        }
    }

    #[test]
    fn run_helpers_atomic_commit_daemon_session_id_alone_marks_csa_subprocess() {
        let _daemon_guard =
            ScopedTestEnvVar::set("CSA_DAEMON_SESSION_ID", "01KPTB1TSQ89AT5GVH8PCZ2SP4");
        let _depth_guard = ScopedEnvVarRestore::unset("CSA_DEPTH");
        let _session_guard = ScopedEnvVarRestore::unset("CSA_SESSION_ID");

        assert!(is_csa_subprocess());
    }

    #[test]
    fn run_helpers_atomic_commit_preamble_uses_exported_canonical_session_output_path() {
        let exec = Executor::GeminiCli {
            model_override: None,
            thinking_budget: None,
        };
        let session = make_test_session();
        let (cmd, _stdin_data) = exec.build_command("hello", None, &session, None);

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        let env_map: HashMap<&OsStr, Option<&OsStr>> = envs.into_iter().collect();
        let session_dir = env_map
            .get(OsStr::new(CSA_SESSION_DIR_ENV_KEY))
            .expect("CSA_SESSION_DIR should be present")
            .expect("CSA_SESSION_DIR should have a value")
            .to_string_lossy();

        for preamble in [
            super::ATOMIC_COMMIT_DISCIPLINE_PREAMBLE,
            super::ATOMIC_COMMIT_DISCIPLINE_SUBPROCESS_PREAMBLE,
        ] {
            assert!(
                preamble.contains("Write deliverables the caller must read after session end"),
                "preamble should use the complete deliverable-persistence sentence"
            );
            assert!(
                preamble.contains("$CSA_SESSION_DIR/output/<name>.md"),
                "preamble should point to the canonical session output artifact path"
            );
            assert!(
                preamble.contains("session output directory IS persisted"),
                "preamble should retain the persisted-output anchor phrase"
            );
            assert!(
                preamble.contains("canonical location for cross-session artifacts"),
                "preamble should clarify that the persisted output path is canonical"
            );
        }

        assert!(
            session_dir.contains("/sessions/"),
            "CSA_SESSION_DIR should contain the session path segment, got: {session_dir}"
        );
        assert!(
            session_dir.contains("01HTEST000000000000000000"),
            "CSA_SESSION_DIR should include the session ID, got: {session_dir}"
        );
    }
}
