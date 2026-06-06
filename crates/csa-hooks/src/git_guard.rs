//! Git guard: deterministic gate preventing hook bypass on `git commit`.
//!
//! CSA tool subprocesses share the caller's `.git` directory, so Git hooks are
//! the last deterministic local gate before an agent-created commit lands in
//! the repository.  This module injects a `git` wrapper ahead of the real Git
//! binary in `PATH`, strips hook-bypass inputs from commits, and blocks leaf
//! worker pushes.

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};

const FALLBACK_GUARD_DIR_NAME: &str = "guards";
const SESSION_GUARD_DIR_NAME: &str = "bin";
const CSA_SESSION_DIR_ENV: &str = "CSA_SESSION_DIR";
static GUARD_SETUP_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
const GIT_WRAPPER: &str = r#"#!/bin/sh
# CSA git guard: strips git commit hook bypass and blocks leaf-worker pushes.
# Injected by CSA via PATH.
set -eu

REAL_GIT="${CSA_REAL_GIT:-}"
if [ -z "${REAL_GIT}" ]; then
  GUARD_DIR="$(cd "$(dirname "$0")" && pwd)"
  CLEAN_PATH=""
  OLD_IFS="${IFS}"
  IFS=:
  for _dir in ${PATH:-}; do
    [ -z "${_dir}" ] && continue
    [ "${_dir}" = "${GUARD_DIR}" ] && continue
    CLEAN_PATH="${CLEAN_PATH:+${CLEAN_PATH}:}${_dir}"
  done
  IFS="${OLD_IFS}"
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

strip_hook_bypass_env() {
  unset LEFTHOOK || true
  unset LEFTHOOK_DISABLED || true
  unset HUSKY || true
  unset HUSKY_DISABLE || true
  unset SKIP_HOOKS || true
  unset SKIP_GIT_HOOKS || true
  unset PRE_COMMIT_ALLOW_NO_CONFIG || true
  unset LEFTHOOK_SKIP || true
  unset LEFTHOOK_EXCLUDE || true
  unset SKIP || true

  for env_name in $(env | sed -n \
    -e 's/^\(LEFTHOOK_SKIP_[A-Za-z0-9_]*\)=.*/\1/p' \
    -e 's/^\(LEFTHOOK_EXCLUDE_[A-Za-z0-9_]*\)=.*/\1/p'); do
    unset "${env_name}" || true
  done
}

append_sanitized_arg() {
  quoted_arg="'$(printf "%s" "$1" | sed "s/'/'\\\\''/g")'"
  if [ -z "${SANITIZED_ARGS}" ]; then
    SANITIZED_ARGS="${quoted_arg}"
  else
    SANITIZED_ARGS="${SANITIZED_ARGS} ${quoted_arg}"
  fi
}

strip_commit_short_arg() {
  short_options="${1#-}"
  rebuilt_options=""
  while [ -n "${short_options}" ]; do
    short_remainder="${short_options#?}"
    short_option="${short_options%"${short_remainder}"}"
    short_options="${short_remainder}"
    case "${short_option}" in
      n)
        STRIPPED_NO_VERIFY=true
        ;;
      C|F|c|m|t)
        rebuilt_options="${rebuilt_options}${short_option}${short_options}"
        short_options=""
        ;;
      *)
        rebuilt_options="${rebuilt_options}${short_option}"
        ;;
    esac
  done

  if [ -n "${rebuilt_options}" ]; then
    STRIPPED_SHORT_RESULT="-${rebuilt_options}"
  else
    STRIPPED_SHORT_RESULT=""
  fi
}

COMMAND=""
BLOCK_HOOKS_PATH_OVERRIDE=false
EXPECT_VALUE=""
for arg do
  if [ -n "${EXPECT_VALUE}" ]; then
    if [ "${EXPECT_VALUE}" = "config" ] && is_hooks_path_config "${arg}"; then
      BLOCK_HOOKS_PATH_OVERRIDE=true
    fi
    EXPECT_VALUE=""
    continue
  fi

  case "${arg}" in
    --help|-h)
      continue
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

strip_hook_bypass_env

if [ "${COMMAND}" = "push" ] && [ "${CSA_GIT_PUSH_ALLOWED:-}" != "true" ]; then
  echo "CSA git-guard: git push blocked for leaf-worker sessions." >&2
  exit 128
fi

if [ "${COMMAND}" != "commit" ]; then
  exec "${REAL_GIT}" "$@"
fi

if [ "${BLOCK_HOOKS_PATH_OVERRIDE}" = "true" ]; then
  echo "BLOCKED: CSA git guard prohibits overriding core.hooksPath for git commit." >&2
  exit 1
fi

SANITIZED_ARGS=""
STRIPPED_NO_VERIFY=false
COMMAND_SEEN=false
END_OF_COMMIT_OPTIONS=false
EXPECT_GLOBAL_VALUE=""
EXPECT_COMMIT_VALUE=""
for arg do
  if [ "${COMMAND_SEEN}" = "false" ]; then
    append_sanitized_arg "${arg}"
    if [ -n "${EXPECT_GLOBAL_VALUE}" ]; then
      EXPECT_GLOBAL_VALUE=""
      continue
    fi

    case "${arg}" in
      -c|--config|-C|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix|--config-env)
        EXPECT_GLOBAL_VALUE="1"
        continue
        ;;
      --config=*|--git-dir=*|--work-tree=*|--namespace=*|--exec-path=*|--super-prefix=*|--config-env=*)
        continue
        ;;
      --*)
        continue
        ;;
      -*)
        continue
        ;;
      *)
        [ "${arg}" = "commit" ] && COMMAND_SEEN=true
        continue
        ;;
    esac
  fi

  if [ "${END_OF_COMMIT_OPTIONS}" = "true" ]; then
    append_sanitized_arg "${arg}"
    continue
  fi

  if [ -n "${EXPECT_COMMIT_VALUE}" ]; then
    append_sanitized_arg "${arg}"
    EXPECT_COMMIT_VALUE=""
    continue
  fi

  case "${arg}" in
    --no-verify|--no-verify=*)
      STRIPPED_NO_VERIFY=true
      ;;
    --)
      append_sanitized_arg "${arg}"
      END_OF_COMMIT_OPTIONS=true
      continue
      ;;
    --author|--cleanup|--date|--file|--fixup|--message|--pathspec-from-file|--reuse-message|--reedit-message|--squash|--template|--trailer)
      append_sanitized_arg "${arg}"
      EXPECT_COMMIT_VALUE="commit-option"
      continue
      ;;
    --author=*|--cleanup=*|--date=*|--file=*|--fixup=*|--message=*|--pathspec-from-file=*|--reuse-message=*|--reedit-message=*|--squash=*|--template=*|--trailer=*)
      append_sanitized_arg "${arg}"
      continue
      ;;
    --*)
      append_sanitized_arg "${arg}"
      ;;
    -*)
      STRIPPED_SHORT_RESULT=""
      strip_commit_short_arg "${arg}"
      if [ -n "${STRIPPED_SHORT_RESULT}" ]; then
        append_sanitized_arg "${STRIPPED_SHORT_RESULT}"
      fi
      case "${STRIPPED_SHORT_RESULT}" in
        -*[CFcmt])
          EXPECT_COMMIT_VALUE="commit-option"
          ;;
      esac
      ;;
    *)
      append_sanitized_arg "${arg}"
      ;;
  esac
done

if [ "${STRIPPED_NO_VERIFY}" = "true" ]; then
  echo "CSA git-guard: stripped --no-verify from git commit (hook bypass forbidden)" >&2
fi

if "${REAL_GIT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  top="$("${REAL_GIT}" rev-parse --show-toplevel 2>/dev/null || pwd)"
  lefthook_config=""
  for cfg in lefthook.yml lefthook.yaml .lefthook.yml .lefthook.yaml; do
    if [ -f "${top}/${cfg}" ]; then
      lefthook_config="${top}/${cfg}"
      break
    fi
  done

  if [ -n "${lefthook_config}" ]; then
    hooks_path="$("${REAL_GIT}" config --path --get core.hooksPath 2>/dev/null || true)"
    hook_path_for() {
      hook_name="$1"
      if [ -n "${hooks_path}" ]; then
        case "${hooks_path}" in
          /*) printf '%s\n' "${hooks_path}/${hook_name}" ;;
          *) printf '%s\n' "${top}/${hooks_path}/${hook_name}" ;;
        esac
      else
        "${REAL_GIT}" rev-parse --git-path "hooks/${hook_name}" 2>/dev/null || true
      fi
    }

    while IFS= read -r line || [ -n "${line}" ]; do
      case "${line}" in
        ""|\#*|" "*) continue ;;
      esac
      hook_name="${line%%:*}"
      [ "${hook_name}" = "${line}" ] && continue
      case "${hook_name}" in
        applypatch-msg|commit-msg|fsmonitor-watchman|p4-changelist|p4-post-changelist|p4-pre-submit|p4-prepare-changelist|post-applypatch|post-checkout|post-commit|post-index-change|post-merge|post-receive|post-rewrite|post-update|pre-applypatch|pre-auto-gc|pre-commit|pre-merge-commit|pre-push|pre-rebase|pre-receive|prepare-commit-msg|proc-receive|push-to-checkout|reference-transaction|sendemail-validate|update)
          hook_path="$(hook_path_for "${hook_name}")"
          if [ -z "${hook_path}" ] || [ ! -x "${hook_path}" ]; then
            echo "BLOCKED: lefthook config defines ${hook_name} but no executable ${hook_name} hook is active." >&2
            echo "Run: lefthook install" >&2
            if [ -n "${hook_path}" ]; then
              echo "Expected hook: ${hook_path}" >&2
            fi
            exit 1
          fi
          ;;
      esac
    done < "${lefthook_config}"
  fi
fi

eval "set -- ${SANITIZED_ARGS}"
exec "${REAL_GIT}" "$@"
"#;

pub fn ensure_git_guard_dir() -> Result<PathBuf> {
    let data_dir = csa_config::paths::state_dir()
        .context("cannot determine CSA state directory")?
        .join(FALLBACK_GUARD_DIR_NAME);

    ensure_git_guard_dir_at(&data_dir)
}

fn ensure_git_guard_dir_for_env(env: &HashMap<String, String>) -> Result<PathBuf> {
    if let Some(session_dir) = env
        .get(CSA_SESSION_DIR_ENV)
        .filter(|value| !value.is_empty())
    {
        return ensure_git_guard_dir_at(&Path::new(session_dir).join(SESSION_GUARD_DIR_NAME));
    }

    ensure_git_guard_dir()
}

fn ensure_git_guard_dir_at(data_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create guard dir: {}", data_dir.display()))?;
    let wrapper_path = data_dir.join("git");
    fs::write(&wrapper_path, GIT_WRAPPER)
        .with_context(|| format!("failed to write git wrapper: {}", wrapper_path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod git wrapper: {}", wrapper_path.display()))?;

    Ok(data_dir.to_path_buf())
}

pub fn inject_git_guard_env(env: &mut HashMap<String, String>) {
    let _setup_lock = match GUARD_SETUP_LOCK.lock() {
        Ok(lock) => lock,
        Err(error) => {
            tracing::warn!("git guard setup lock poisoned (best-effort skip): {error}");
            return;
        }
    };
    let guard_dir = match ensure_git_guard_dir_for_env(env) {
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
