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

is_alias_config() {
  case "${1%%=*}" in
    [Aa][Ll][Ii][Aa][Ss].*) return 0 ;;
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
ALIAS_CONFIG_OVERRIDE=false
EXPECT_VALUE=""
for arg do
  if [ -n "${EXPECT_VALUE}" ]; then
    if [ "${EXPECT_VALUE}" = "config" ]; then
      if is_hooks_path_config "${arg}"; then
        BLOCK_HOOKS_PATH_OVERRIDE=true
      fi
      if is_alias_config "${arg}"; then
        ALIAS_CONFIG_OVERRIDE=true
      fi
    fi
    EXPECT_VALUE=""
    continue
  fi

  case "${arg}" in
    --help|-h)
      continue
      ;;
    --version*)
      COMMAND="${arg}"
      break
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
      if is_alias_config "${value}"; then
        ALIAS_CONFIG_OVERRIDE=true
      fi
      continue
      ;;
    -ccore.hooksPath=*|-ccore.hookspath=*)
      BLOCK_HOOKS_PATH_OVERRIDE=true
      continue
      ;;
    -calias.*=-*|-calias.*=*|-c[Aa][Ll][Ii][Aa][Ss].*=*)
      ALIAS_CONFIG_OVERRIDE=true
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

canonical_directory() {
  CDPATH= cd "${1}" 2>/dev/null && pwd -P
}

is_descendant_of() {
  case "${1}" in
    "${2}"/*) return 0 ;;
    *) return 1 ;;
  esac
}

has_control_bytes() {
  case "${1}" in
    *[!\ -~]*) return 0 ;;
    *) return 1 ;;
  esac
}

descriptor_path_for() {
  descriptor_number="${1}"
  for descriptor_root in /proc/self/fd /dev/fd; do
    if [ -d "${descriptor_root}/${descriptor_number}" ]; then
      printf '%s\n' "${descriptor_root}/${descriptor_number}"
      return 0
    fi
  done
  return 1
}

reset_hermetic_git_environment() {
  # Hermetic Git operations must not inherit caller-selected repository,
  # template, config, initialization defaults, trace output, protocol, or
  # helper-routing inputs. Projected pushes also run from an empty temporary
  # Git directory, excluding local remotes and URL rewrites from transport
  # decisions.
  unset GIT_CONFIG GIT_CONFIG_PARAMETERS GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_NAMESPACE || true
  unset GIT_CEILING_DIRECTORIES GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_TEMPLATE_DIR || true
  unset GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES || true
  unset GIT_EXEC_PATH GIT_SSH GIT_SSH_COMMAND GIT_SSH_VARIANT GIT_PROXY_COMMAND || true
  unset GIT_ASKPASS SSH_ASKPASS GIT_TRANSPORT_HELPER_DIR GIT_REMOTE_HELPER_DIR || true
  for env_name in $(env | sed -n \
    -e 's/^\(GIT_CONFIG_[A-Za-z0-9_]*\)=.*/\1/p' \
    -e 's/^\(GIT_DEFAULT_[A-Za-z0-9_]*\)=.*/\1/p' \
    -e 's/^\(GIT_TEST_DEFAULT_[A-Za-z0-9_]*\)=.*/\1/p' \
    -e 's/^\(GIT_TRACE[A-Za-z0-9_]*\)=.*/\1/p'); do
    unset "${env_name}" || true
  done

  export GIT_CONFIG_NOSYSTEM=1
  export GIT_CONFIG_SYSTEM=/dev/null
  export GIT_CONFIG_GLOBAL=/dev/null
  export XDG_CONFIG_HOME=/dev/null

  trusted_exec_path="$("${REAL_GIT}" --exec-path 2>/dev/null)" || return 1
  case "${trusted_exec_path}" in
    /*) ;;
    *) return 1 ;;
  esac
  trusted_exec_path="$(canonical_directory "${trusted_exec_path}")" || return 1

  export GIT_ALLOW_PROTOCOL=file
  export GIT_PROTOCOL_FROM_USER=0
  export GIT_EXEC_PATH="${trusted_exec_path}"
}

create_hermetic_push_projection() {
  # Git has no switch to ignore only the source repository's config. Build a
  # minimal, configless source snapshot instead: object alternates plus copied
  # refs give real pack negotiation without allowing local remote/pushurl or
  # URL-rewrite settings into the final invocation.
  source_common_dir="$("${REAL_GIT}" rev-parse --git-common-dir 2>/dev/null)" || return 1
  source_common_dir="$(canonical_directory "${source_common_dir}")" || return 1
  source_objects="${source_common_dir}/objects"
  [ -d "${source_objects}" ] || return 1
  source_head="$("${REAL_GIT}" rev-parse --verify HEAD 2>/dev/null)" || return 1

  umask 077
  HERMETIC_PUSH_PROJECTION="$(mktemp -d "${session_dir}/.git-guard-projection.XXXXXX")" || return 1
  mkdir -p "${HERMETIC_PUSH_PROJECTION}/objects/info" "${HERMETIC_PUSH_PROJECTION}/refs" || return 1
  : > "${HERMETIC_PUSH_PROJECTION}/config" || return 1
  printf '%s\n' "${source_objects}" > "${HERMETIC_PUSH_PROJECTION}/objects/info/alternates" || return 1
  printf '%s\n' "${source_head}" > "${HERMETIC_PUSH_PROJECTION}/HEAD" || return 1

  refs_snapshot="${HERMETIC_PUSH_PROJECTION}/refs.snapshot"
  "${REAL_GIT}" for-each-ref --format='%(refname) %(objectname)' > "${refs_snapshot}" || return 1
  while IFS=' ' read -r refname object_id remainder || [ -n "${refname:-}" ]; do
    [ -n "${refname:-}" ] || continue
    [ -n "${object_id:-}" ] || return 1
    [ -z "${remainder:-}" ] || return 1
    "${REAL_GIT}" --git-dir="${HERMETIC_PUSH_PROJECTION}" update-ref "${refname}" "${object_id}" || return 1
  done < "${refs_snapshot}"
  rm -f "${refs_snapshot}"
}

cleanup_hermetic_push_projection() {
  if [ -n "${HERMETIC_PUSH_PROJECTION:-}" ]; then
    rm -rf "${HERMETIC_PUSH_PROJECTION}"
    HERMETIC_PUSH_PROJECTION=""
  fi
}

is_hermetic_local_bare_push() {
  # Closed grammar for the one fixture exception: no global options, exactly
  # `push <absolute-path|file:///local> <non-force-refspec...>`. Requiring a
  # refspec also prevents repository push defaults from becoming policy input.
  [ "${1:-}" = "push" ] || return 1
  [ "$#" -ge 3 ] || return 1
  destination="${2}"
  shift 2
  has_control_bytes "${destination}" && return 1
  case "${destination}" in
    file:///*) destination_path="${destination#file://}" ;;
    /*) destination_path="${destination}" ;;
    *) return 1 ;;
  esac
  for refspec do
    has_control_bytes "${refspec}" && return 1
    case "${refspec}" in
      ""|-*|+*|:*) return 1 ;;
    esac
  done

  reset_hermetic_git_environment || return 1

  session_dir="${CSA_SESSION_DIR:-}"
  [ -n "${session_dir}" ] || return 1
  session_dir="$(canonical_directory "${session_dir}")" || return 1
  guard_dir="$(canonical_directory "$(dirname "$0")")" || return 1
  expected_guard_dir="$(canonical_directory "${session_dir}/bin")" || return 1
  [ "${guard_dir}" = "${expected_guard_dir}" ] || return 1

  fixture_root="$(canonical_directory "${session_dir}/git-fixtures")" || return 1
  is_descendant_of "${fixture_root}" "${session_dir}" || return 1
  destination_path="$(canonical_directory "${destination_path}")" || return 1
  is_descendant_of "${destination_path}" "${fixture_root}" || return 1

  # A fixture must never be a ref/remote location inside the worktree that is
  # being guarded. Check both the executing worktree and CSA's session root.
  if worktree="$("${REAL_GIT}" rev-parse --show-toplevel 2>/dev/null)"; then
    worktree="$(canonical_directory "${worktree}")" || return 1
    is_descendant_of "${destination_path}" "${worktree}" && return 1
  fi
  if [ -n "${CSA_PROJECT_ROOT:-}" ]     && project_root="$(canonical_directory "${CSA_PROJECT_ROOT}")"; then
    is_descendant_of "${destination_path}" "${project_root}" && return 1
  fi

  # Keep both the fixture root and bare destination open. Re-canonicalizing
  # their descriptor paths after open makes a concurrent symlink/rename swap
  # fail closed; the transfer receives the pinned destination descriptor.
  exec 8<"${fixture_root}" || return 1
  exec 9<"${destination_path}" || return 1
  fixture_root_descriptor="$(descriptor_path_for 8)" || return 1
  destination_descriptor="$(descriptor_path_for 9)" || return 1
  fixture_root="$(canonical_directory "${fixture_root_descriptor}")" || return 1
  destination_path="$(canonical_directory "${destination_descriptor}")" || return 1
  is_descendant_of "${fixture_root}" "${session_dir}" || return 1
  is_descendant_of "${destination_path}" "${fixture_root}" || return 1
  [ "$("${REAL_GIT}" -C "${destination_descriptor}" rev-parse --is-bare-repository 2>/dev/null)" = "true" ] || return 1

  HERMETIC_PUSH_DESTINATION="${destination_descriptor}"
  create_hermetic_push_projection || return 1
  return 0
}

format_git_command() {
  # Diagnostics are intentionally structural: a blocked command must never
  # echo user-controlled destination, refspec, path, query, or config values.
  command_seen=false
  expected_value=false
  printf 'git'
  for arg do
    if [ "${expected_value}" = "true" ]; then
      printf ' <value>'
      expected_value=false
      continue
    fi
    case "${arg}" in
      -c|--config|--config-env|-C|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix)
        printf ' %s' "${arg}"
        expected_value=true
        ;;
      --config=*|--config-env=*|--git-dir=*|--work-tree=*|--namespace=*|--exec-path=*|--super-prefix=*)
        printf ' %s=<value>' "${arg%%=*}"
        ;;
      --help|-h)
        printf ' %s' "${arg}"
        ;;
      --*)
        printf ' <option>'
        ;;
      -*)
        printf ' <option>'
        ;;
      *)
        if [ "${command_seen}" = "false" ]; then
          case "${arg}" in
            push|send-pack|receive-pack|status|add|diff|log|commit) printf ' %s' "${arg}" ;;
            *) printf ' <command>' ;;
          esac
          command_seen=true
        else
          printf ' <argument>'
        fi
        ;;
    esac
  done
}

is_exact_git_version_grammar() {
  [ "$#" -eq 1 ] || return 1
  case "${1}" in
    version|--version) return 0 ;;
    *) return 1 ;;
  esac
}

is_exact_git_init_grammar() {
  [ "${1:-}" = "init" ] || return 1
  case "$#" in
    1) return 0 ;;
    2)
      case "${2}" in
        -q|--quiet) return 0 ;;
      esac
      ;;
  esac
  return 1
}

is_safe_local_command() {
  case "${1}" in
    ""|add|am|annotate|apply|bisect|blame|branch|cat-file|check-attr|check-ignore|check-mailmap|check-ref-format|checkout|cherry|cherry-pick|clean|commit|config|count-objects|describe|diff|difftool|fast-export|fast-import|format-patch|fsck|gc|grep|hash-object|log|ls-files|ls-tree|merge|merge-base|merge-file|merge-index|merge-tree|mv|name-rev|notes|pack-objects|prune|read-tree|rebase|reflog|reset|restore|rev-list|rev-parse|rm|show|show-branch|sparse-checkout|stash|status|switch|symbolic-ref|tag|update-index|update-ref|verify-commit|verify-pack|verify-tag|worktree|write-tree)
      return 0
      ;;
    *) return 1 ;;
  esac
}

block_untrusted_git_command() {
  rejected_command="$(format_git_command "$@")"
  echo "CSA git-guard: blocked command: ${rejected_command}" >&2
  echo "Use a hermetic local bare fixture under \$CSA_SESSION_DIR/git-fixtures and push to its direct canonical path or file:// URL." >&2
  exit 128
}

if [ "${CSA_GIT_PUSH_ALLOWED:-}" != "true" ]; then
  # Aliases are expanded by Git after this wrapper chooses a command. Reject
  # caller-supplied alias definitions and fail closed for unknown subcommands,
  # so `git -c alias.publish=push publish …` cannot re-enter publication.
  if [ "${ALIAS_CONFIG_OVERRIDE}" = "true" ]; then
    block_untrusted_git_command "$@"
  fi

  if [ "${COMMAND}" = "push" ]; then
    if ! is_hermetic_local_bare_push "$@"; then
      cleanup_hermetic_push_projection
      block_untrusted_git_command "$@"
    fi

    # Execute only the pinned, configless projection. The original destination,
    # global options, source config, and helper routing are absent from this
    # invocation; refspecs are from the closed grammar above.
    shift 2
    if "${REAL_GIT}" --git-dir="${HERMETIC_PUSH_PROJECTION}" push "${HERMETIC_PUSH_DESTINATION}" "$@"; then
      push_status=0
    else
      push_status=$?
    fi
    cleanup_hermetic_push_projection
    exit "${push_status}"
  fi

  # Closed probes and local fixture bootstrap forms are admitted only with their
  # exact argv; path/config steering and extra operands still fail closed.
  if ! is_exact_git_version_grammar "$@" \
    && ! is_exact_git_init_grammar "$@" \
    && ! is_safe_local_command "${COMMAND}"; then
    block_untrusted_git_command "$@"
  fi
fi

# Every closed-grammar route that forwards caller argv uses the same hermetic
# environment; the local-push route applies it inside its projection validator.
if is_exact_git_version_grammar "$@" || is_exact_git_init_grammar "$@"; then
  reset_hermetic_git_environment || block_untrusted_git_command "$@"
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
    // The guard's child environment is attacker-controlled. Resolve a known
    // host Git rather than inheriting an ambient CSA_REAL_GIT override; this
    // is the trust anchor used to pin the projected transfer's libexec path.
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
