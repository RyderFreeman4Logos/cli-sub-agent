//! Rust toolchain state-path resolution and env override logic for the isolation plan.
//!
//! When `CARGO_HOME` or `RUSTUP_HOME` points at a read-only system prefix
//! (typically `/usr/local`), the sandbox must both (a) add the writable
//! default (`~/.cargo` / `~/.rustup`) to `writable_paths`, and (b) override
//! the env var itself in `env_overrides` so the child process uses the
//! writable path instead of the original read-only one (#2607).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolve a Rust state path, falling back to the default when the original
/// needs session override.
pub(crate) fn resolve_rust_state_path(value: &str, default: &Path) -> PathBuf {
    let path = PathBuf::from(value);
    if value.trim().is_empty() || csa_core::env::rust_state_path_needs_session_override(&path) {
        default.to_path_buf()
    } else {
        path
    }
}

/// If the original env value pointed at a read-only system prefix, insert an
/// override mapping it to the writable default. Returns `true` when an
/// override was inserted.
pub(crate) fn insert_env_override_if_needed(
    env_overrides: &mut HashMap<String, String>,
    env_key: &str,
    original_value: &str,
    default_path: &Path,
) -> bool {
    if csa_core::env::rust_state_path_needs_session_override(&PathBuf::from(original_value)) {
        env_overrides.insert(
            env_key.to_string(),
            default_path.to_string_lossy().into_owned(),
        );
        true
    } else {
        false
    }
}
