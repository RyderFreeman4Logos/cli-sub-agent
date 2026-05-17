//! Caller session auto-detection for CSA-lite fork (issue #1432).
//!
//! Discovers the *caller's* Claude session — the conversation that
//! invoked CSA — so a fork operation can reload the caller's history.
//! Detection prefers the zero-cost `CLAUDE_SESSION_ID` env var; if
//! unset, falls back to a `xurl_core` query for the most recently
//! updated Claude thread on disk.

use std::env;
use std::path::PathBuf;

use tracing::debug;
use xurl_core::{AgentsUri, ProviderKind, ProviderRoots, ThreadQuery, resolve_thread};

const CLAUDE_SESSION_ID_ENV: &str = "CLAUDE_SESSION_ID";
const CLAUDE_PROVIDER: &str = "claude";

/// Information about a caller session discovered on disk.
///
/// `session_dir` is the directory containing `jsonl_path` — for Claude
/// this is `~/.claude/projects/<encoded-project>/`. Callers needing
/// per-session state (e.g. shadow checkpoints) should derive it from
/// `jsonl_path` rather than assume one-directory-per-session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerSessionInfo {
    pub session_id: String,
    pub jsonl_path: PathBuf,
    pub session_dir: PathBuf,
    pub provider: String,
}

/// Detect the caller's session, preferring the `CLAUDE_SESSION_ID`
/// env var over an `xurl_core`-based latest-thread fallback.
///
/// Returns `None` when no session can be resolved (env unset and no
/// Claude threads on disk, or the resolved JSONL file is missing).
pub fn detect_caller_session() -> Option<CallerSessionInfo> {
    if let Some(info) = detect_from_env() {
        debug!(
            session_id = %info.session_id,
            jsonl = %info.jsonl_path.display(),
            "caller session detected via CLAUDE_SESSION_ID"
        );
        return Some(info);
    }

    if let Some(info) = detect_from_xurl_latest() {
        debug!(
            session_id = %info.session_id,
            jsonl = %info.jsonl_path.display(),
            "caller session detected via xurl latest-thread fallback"
        );
        return Some(info);
    }

    debug!("no caller session detected");
    None
}

fn detect_from_env() -> Option<CallerSessionInfo> {
    let raw = env::var(CLAUDE_SESSION_ID_ENV).ok()?;
    let session_id = raw.trim();
    if session_id.is_empty() {
        debug!("CLAUDE_SESSION_ID is empty; skipping env detection");
        return None;
    }

    let roots = match ProviderRoots::from_env_or_home() {
        Ok(roots) => roots,
        Err(err) => {
            debug!(error = %err, "failed to resolve provider roots");
            return None;
        }
    };

    let uri_str = format!("{CLAUDE_PROVIDER}://{session_id}");
    let uri: AgentsUri = match uri_str.parse() {
        Ok(uri) => uri,
        Err(err) => {
            debug!(uri = %uri_str, error = %err, "failed to parse claude URI");
            return None;
        }
    };

    let resolved = match resolve_thread(&uri, &roots) {
        Ok(resolved) => resolved,
        Err(err) => {
            debug!(session_id = %session_id, error = %err, "xurl could not resolve session");
            return None;
        }
    };

    build_info(resolved.session_id, resolved.path)
}

fn detect_from_xurl_latest() -> Option<CallerSessionInfo> {
    let roots = ProviderRoots::from_env_or_home().ok()?;
    let query = ThreadQuery {
        uri: format!("{CLAUDE_PROVIDER}://"),
        provider: ProviderKind::Claude,
        role: None,
        q: None,
        limit: 1,
        ignored_params: Vec::new(),
    };

    let result = xurl_core::query_threads(&query, &roots).ok()?;
    let item = result.items.into_iter().next()?;
    build_info(item.thread_id, PathBuf::from(item.thread_source))
}

fn build_info(session_id: String, jsonl_path: PathBuf) -> Option<CallerSessionInfo> {
    if !jsonl_path.is_file() {
        debug!(
            jsonl = %jsonl_path.display(),
            "resolved JSONL path is not a file; rejecting"
        );
        return None;
    }
    let session_dir = jsonl_path.parent()?.to_path_buf();
    Some(CallerSessionInfo {
        session_id,
        jsonl_path,
        session_dir,
        provider: CLAUDE_PROVIDER.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::TEST_ENV_LOCK;
    use std::fs;
    use tempfile::TempDir;

    /// RAII guard that sets env vars on construction and clears them on drop.
    /// Holds TEST_ENV_LOCK to serialize tests that mutate process-wide env.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[(&str, Option<&str>)]) -> Self {
            let lock = TEST_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut previous = Vec::new();
            for (key, value) in vars {
                previous.push(((*key).to_string(), env::var(key).ok()));
                match value {
                    // SAFETY: serialized by TEST_ENV_LOCK.
                    Some(v) => unsafe { env::set_var(key, v) },
                    // SAFETY: serialized by TEST_ENV_LOCK.
                    None => unsafe { env::remove_var(key) },
                }
            }
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, prev) in &self.previous {
                match prev {
                    // SAFETY: serialized by TEST_ENV_LOCK.
                    Some(v) => unsafe { env::set_var(key, v) },
                    // SAFETY: serialized by TEST_ENV_LOCK.
                    None => unsafe { env::remove_var(key) },
                }
            }
        }
    }

    /// Build a fake `~/.claude/projects/<encoded>/<session>.jsonl`
    /// tree under `tempdir`, returning the JSONL path.
    fn seed_claude_session(tempdir: &TempDir, session_id: &str) -> PathBuf {
        let projects = tempdir.path().join("projects/-fake-project");
        fs::create_dir_all(&projects).expect("create projects dir");
        let jsonl = projects.join(format!("{session_id}.jsonl"));
        let header = serde_json::json!({
            "type": "summary",
            "sessionId": session_id,
        });
        fs::write(&jsonl, format!("{header}\n")).expect("write jsonl");
        jsonl
    }

    #[test]
    fn env_var_set_with_valid_session_returns_some() {
        let tempdir = TempDir::new().expect("tempdir");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let jsonl = seed_claude_session(&tempdir, session_id);

        let claude_root = tempdir.path().to_string_lossy().to_string();
        let _guard = EnvGuard::new(&[
            (CLAUDE_SESSION_ID_ENV, Some(session_id)),
            ("CLAUDE_CONFIG_DIR", Some(claude_root.as_str())),
        ]);

        let info = detect_caller_session().expect("session should resolve");
        assert_eq!(info.session_id, session_id);
        assert_eq!(info.jsonl_path, jsonl);
        assert_eq!(info.session_dir, jsonl.parent().unwrap());
        assert_eq!(info.provider, CLAUDE_PROVIDER);
    }

    #[test]
    fn env_var_set_with_missing_session_returns_none() {
        let tempdir = TempDir::new().expect("tempdir");
        fs::create_dir_all(tempdir.path().join("projects")).expect("mkdir");

        let claude_root = tempdir.path().to_string_lossy().to_string();
        let _guard = EnvGuard::new(&[
            (
                CLAUDE_SESSION_ID_ENV,
                Some("00000000-0000-0000-0000-000000000000"),
            ),
            ("CLAUDE_CONFIG_DIR", Some(claude_root.as_str())),
        ]);

        assert!(detect_caller_session().is_none());
    }

    #[test]
    fn env_var_empty_falls_through_to_fallback() {
        let tempdir = TempDir::new().expect("tempdir");
        fs::create_dir_all(tempdir.path().join("projects")).expect("mkdir");

        let claude_root = tempdir.path().to_string_lossy().to_string();
        let _guard = EnvGuard::new(&[
            (CLAUDE_SESSION_ID_ENV, Some("")),
            ("CLAUDE_CONFIG_DIR", Some(claude_root.as_str())),
        ]);

        // No sessions seeded → fallback returns None too.
        assert!(detect_caller_session().is_none());
    }

    #[test]
    fn env_var_unset_with_no_sessions_returns_none() {
        let tempdir = TempDir::new().expect("tempdir");
        fs::create_dir_all(tempdir.path().join("projects")).expect("mkdir");

        let claude_root = tempdir.path().to_string_lossy().to_string();
        let _guard = EnvGuard::new(&[
            (CLAUDE_SESSION_ID_ENV, None),
            ("CLAUDE_CONFIG_DIR", Some(claude_root.as_str())),
        ]);

        assert!(detect_caller_session().is_none());
    }

    #[test]
    fn build_info_rejects_nonfile_path() {
        let tempdir = TempDir::new().expect("tempdir");
        let missing = tempdir.path().join("nope.jsonl");
        assert!(build_info("sid".to_string(), missing).is_none());
    }
}
