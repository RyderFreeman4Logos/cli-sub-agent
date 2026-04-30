//! Git guard: deterministic gate preventing hook bypass on `git commit`.
//!
//! CSA tool subprocesses share the caller's `.git` directory, so Git hooks are
//! the last deterministic local gate before an agent-created commit lands in
//! the repository.  This module injects a `git` wrapper ahead of the real Git
//! binary in `PATH` and blocks known hook-bypass forms before the commit is
//! created.

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};

const GUARD_DIR_NAME: &str = "guards";
static GUARD_SETUP_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
const GIT_WRAPPER: &str = r#"#!/usr/bin/env bash
# CSA git guard: blocks git commit hook bypass.
# Injected by CSA via PATH.
set -euo pipefail

REAL_GIT="${CSA_REAL_GIT:-}"
if [ -z "${REAL_GIT}" ]; then
  GUARD_DIR="$(cd "$(dirname "$0")" && pwd)"
  CLEAN_PATH=""
  IFS=: read -ra _PATH_DIRS <<< "${PATH:-}"
  for _dir in "${_PATH_DIRS[@]}"; do
    [ -z "${_dir}" ] && continue
    [ "${_dir}" = "${GUARD_DIR}" ] && continue
    CLEAN_PATH="${CLEAN_PATH:+${CLEAN_PATH}:}${_dir}"
  done
  REAL_GIT="$(PATH="${CLEAN_PATH}" command -v git 2>/dev/null)" || true
fi

if [ -z "${REAL_GIT}" ]; then
  echo "ERROR: CSA git guard cannot find real git binary." >&2
  exit 1
fi

is_hooks_path_config() {
  case "$1" in
    core.hooksPath=*|core.hookspath=*|hooksPath=*|hookspath=*) return 0 ;;
    *) return 1 ;;
  esac
}

COMMAND=""
BLOCK_HOOKS_PATH_OVERRIDE=false
EXPECT_VALUE=""
for arg in "$@"; do
  if [ -n "${EXPECT_VALUE}" ]; then
    if [ "${EXPECT_VALUE}" = "config" ] && is_hooks_path_config "${arg}"; then
      BLOCK_HOOKS_PATH_OVERRIDE=true
    fi
    EXPECT_VALUE=""
    continue
  fi

  case "${arg}" in
    --help|-h)
      exec "${REAL_GIT}" "$@"
      ;;
    -c|--config)
      EXPECT_VALUE="config"
      continue
      ;;
    --config=*)
      value="${arg#--config=}"
      if is_hooks_path_config "${value}"; then
        BLOCK_HOOKS_PATH_OVERRIDE=true
      fi
      continue
      ;;
    -ccore.hooksPath=*|-ccore.hookspath=*)
      BLOCK_HOOKS_PATH_OVERRIDE=true
      continue
      ;;
    -C|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix|--config-env)
      EXPECT_VALUE="other"
      continue
      ;;
    --git-dir=*|--work-tree=*|--namespace=*|--exec-path=*|--super-prefix=*|--config-env=*)
      continue
      ;;
    --*)
      continue
      ;;
    -*)
      continue
      ;;
    *)
      COMMAND="${arg}"
      break
      ;;
  esac
done

if [ "${COMMAND}" != "commit" ]; then
  exec "${REAL_GIT}" "$@"
fi

if [ "${BLOCK_HOOKS_PATH_OVERRIDE}" = "true" ]; then
  echo "BLOCKED: CSA git guard prohibits overriding core.hooksPath for git commit." >&2
  exit 1
fi

for arg in "$@"; do
  case "${arg}" in
    --no-verify|--no-verify=*)
      echo "BLOCKED: CSA git guard prohibits git commit --no-verify." >&2
      exit 1
      ;;
    --*)
      ;;
    -?*n*|-n)
      # Git commit uses -n as the short form of --no-verify.
      echo "BLOCKED: CSA git guard prohibits git commit -n/short -n combinations." >&2
      exit 1
      ;;
  esac
done

if [ "${LEFTHOOK:-}" = "0" ]; then
  echo "BLOCKED: CSA git guard prohibits LEFTHOOK=0 during git commit." >&2
  exit 1
fi

if [ -n "${LEFTHOOK_SKIP:-}" ] || [ -n "${LEFTHOOK_EXCLUDE:-}" ] || [ -n "${SKIP:-}" ]; then
  echo "BLOCKED: CSA git guard prohibits LEFTHOOK_SKIP/LEFTHOOK_EXCLUDE/SKIP during git commit." >&2
  exit 1
fi

while IFS='=' read -r name _value; do
  case "${name}" in
    LEFTHOOK_SKIP_*|LEFTHOOK_EXCLUDE_*)
      echo "BLOCKED: CSA git guard prohibits ${name} during git commit." >&2
      exit 1
      ;;
  esac
done < <(env)

if "${REAL_GIT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  top="$("${REAL_GIT}" rev-parse --show-toplevel 2>/dev/null || pwd)"
  has_lefthook_config=false
  for cfg in lefthook.yml lefthook.yaml .lefthook.yml .lefthook.yaml; do
    if [ -f "${top}/${cfg}" ]; then
      has_lefthook_config=true
      break
    fi
  done

  if [ "${has_lefthook_config}" = "true" ]; then
    hooks_path="$("${REAL_GIT}" config --path --get core.hooksPath 2>/dev/null || true)"
    if [ -n "${hooks_path}" ]; then
      case "${hooks_path}" in
        /*) pre_commit="${hooks_path}/pre-commit" ;;
        *) pre_commit="${top}/${hooks_path}/pre-commit" ;;
      esac
    else
      pre_commit="$("${REAL_GIT}" rev-parse --git-path hooks/pre-commit 2>/dev/null || true)"
    fi

    if [ -z "${pre_commit}" ] || [ ! -x "${pre_commit}" ]; then
      echo "BLOCKED: lefthook config exists but no executable pre-commit hook is active." >&2
      echo "Run: lefthook install" >&2
      if [ -n "${pre_commit}" ]; then
        echo "Expected hook: ${pre_commit}" >&2
      fi
      exit 1
    fi
  fi
fi

exec "${REAL_GIT}" "$@"
"#;

pub fn ensure_git_guard_dir() -> Result<PathBuf> {
    let data_dir = csa_config::paths::state_dir()
        .context("cannot determine CSA state directory")?
        .join(GUARD_DIR_NAME);

    fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create guard dir: {}", data_dir.display()))?;

    let wrapper_path = data_dir.join("git");
    fs::write(&wrapper_path, GIT_WRAPPER)
        .with_context(|| format!("failed to write git wrapper: {}", wrapper_path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod git wrapper: {}", wrapper_path.display()))?;

    Ok(data_dir)
}

pub fn inject_git_guard_env(env: &mut HashMap<String, String>) {
    let _setup_lock = match GUARD_SETUP_LOCK.lock() {
        Ok(lock) => lock,
        Err(error) => {
            tracing::warn!("git guard setup lock poisoned (best-effort skip): {error}");
            return;
        }
    };
    let guard_dir = match ensure_git_guard_dir() {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!("git guard setup failed (best-effort skip): {error:#}");
            return;
        }
    };

    let real_git = real_git_binary(&guard_dir);
    if let Some(real_git) = real_git {
        env.insert(
            "CSA_REAL_GIT".to_string(),
            real_git.to_string_lossy().into_owned(),
        );
    }

    prepend_guard_dir(env, &guard_dir);
}

fn real_git_binary(guard_dir: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CSA_REAL_GIT").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }

    let guard_dir_str = guard_dir.to_string_lossy();
    let is_not_guard = |path: &Path| !path.to_string_lossy().starts_with(guard_dir_str.as_ref());

    ["/usr/bin/git", "/usr/local/bin/git", "/bin/git"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file() && is_not_guard(path))
        .or_else(|| which::which("git").ok().filter(|path| is_not_guard(path)))
}

fn prepend_guard_dir(env: &mut HashMap<String, String>, guard_dir: &Path) {
    let guard_dir_str = guard_dir.to_string_lossy().into_owned();
    let current_path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let filtered = current_path
        .split(':')
        .filter(|entry| !entry.is_empty() && *entry != guard_dir_str)
        .collect::<Vec<_>>()
        .join(":");
    let new_path = if filtered.is_empty() {
        guard_dir_str
    } else {
        format!("{guard_dir_str}:{filtered}")
    };
    env.insert("PATH".to_string(), new_path);
}

#[must_use]
pub fn git_wrapper_script() -> &'static str {
    GIT_WRAPPER
}

#[cfg(test)]
mod tests {
    use super::{git_wrapper_script, inject_git_guard_env};
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn inject_git_guard_env_sets_real_git_and_path() {
        let mut env = HashMap::new();
        inject_git_guard_env(&mut env);

        assert!(env.contains_key("PATH"));
        assert!(
            env.get("PATH")
                .is_some_and(|value| value.contains("guards"))
        );
        assert!(env.contains_key("CSA_REAL_GIT"));
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_blocks_no_verify_before_real_git() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join("git");
        std::fs::write(&wrapper, git_wrapper_script()).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let fake_git = temp.path().join("real-git");
        let marker = temp.path().join("called");
        std::fs::write(
            &fake_git,
            format!(
                "#!/usr/bin/env bash\necho called > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = std::process::Command::new(&wrapper)
            .arg("commit")
            .arg("--no-verify")
            .env("CSA_REAL_GIT", &fake_git)
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(!marker.exists(), "real git must not run after --no-verify");
        assert!(String::from_utf8_lossy(&output.stderr).contains("--no-verify"));
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_blocks_lefthook_bypass_env_before_real_git() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join("git");
        std::fs::write(&wrapper, git_wrapper_script()).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let fake_git = temp.path().join("real-git");
        let marker = temp.path().join("called");
        std::fs::write(
            &fake_git,
            format!(
                "#!/usr/bin/env bash\necho called > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = std::process::Command::new(&wrapper)
            .arg("commit")
            .arg("-m")
            .arg("test")
            .env("CSA_REAL_GIT", &fake_git)
            .env("LEFTHOOK", "0")
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(!marker.exists(), "real git must not run after LEFTHOOK=0");
        assert!(String::from_utf8_lossy(&output.stderr).contains("LEFTHOOK=0"));
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_allows_long_commit_options_containing_n() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join("git");
        std::fs::write(&wrapper, git_wrapper_script()).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let fake_git = temp.path().join("real-git");
        std::fs::write(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = std::process::Command::new(&wrapper)
            .arg("commit")
            .arg("--amend")
            .env("CSA_REAL_GIT", &fake_git)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "commit --amend"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_forwards_non_commit_commands() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = temp.path().join("git");
        std::fs::write(&wrapper, git_wrapper_script()).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let fake_git = temp.path().join("real-git");
        std::fs::write(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = std::process::Command::new(&wrapper)
            .arg("status")
            .env("CSA_REAL_GIT", &fake_git)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "status");
    }
}
