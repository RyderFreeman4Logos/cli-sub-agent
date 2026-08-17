commit_attempt_fingerprint() {
  head="$("${REAL_GIT}" rev-parse --verify HEAD 2>/dev/null || printf '%s' unborn)"
  index_tree="$("${REAL_GIT}" write-tree 2>/dev/null)" || return 1
  args_hash="$(for arg do printf '%s:%s\n' "${#arg}" "${arg}"; done | "${REAL_GIT}" hash-object --stdin)" || return 1
  env_hash="$(env | LC_ALL=C sort | "${REAL_GIT}" hash-object --stdin)" || return 1
  config_hash="$("${REAL_GIT}" config --null --list --show-origin 2>/dev/null | "${REAL_GIT}" hash-object --stdin)" || return 1
  hooks_hash="$(
    {
      if [ -n "${top}" ]; then
        for hook_name in pre-commit prepare-commit-msg commit-msg; do
          hook_path="$(hook_path_for "${hook_name}")"
          if [ -f "${hook_path}" ]; then
            "${REAL_GIT}" hash-object "${hook_path}" 2>/dev/null || printf '%s\n' unreadable
          else
            printf '%s\n' absent
          fi
        done
      fi
    } | "${REAL_GIT}" hash-object --stdin
  )" || return 1
  printf '%s %s %s %s %s %s\n' "${head}" "${index_tree}" "${args_hash}" "${env_hash}" "${config_hash}" "${hooks_hash}"
}
commit_failure_marker="" commit_fingerprint=""
if [ "${CSA_FS_SANDBOXED:-}" = "1" ] && [ -n "${CSA_SESSION_DIR:-}" ]; then
  session_dir="$(canonical_directory "${CSA_SESSION_DIR}")" || session_dir=""
  guard_dir="$(canonical_directory "$(dirname "$0")")" || guard_dir=""
  expected_guard_dir="$(canonical_directory "${session_dir}/bin")" || expected_guard_dir=""
  if [ -n "${session_dir}" ] && [ "${guard_dir}" = "${expected_guard_dir}" ]; then
    commit_failure_marker="${session_dir}/__CSA_GIT_COMMIT_FAILURE_MARKER__"
    commit_fingerprint="$(commit_attempt_fingerprint "$@" || true)"
    blocked_fingerprint=""
    if [ -f "${commit_failure_marker}" ]; then
      IFS= read -r blocked_fingerprint < "${commit_failure_marker}" || true
    fi
    if [ -n "${commit_fingerprint}" ] && [ "${blocked_fingerprint}" = "${commit_fingerprint}" ]; then
      echo "BLOCKED: hook-enabled commit already failed for this unchanged staged tree in the filesystem sandbox; mandatory hooks will not be rerun." >&2
      echo "The staged tree is preserved. Inspect the first hook failure, then run the same hook-enabled commit outside the sandbox if it needs host resources." >&2
      exit 1
    fi
  fi
fi

if [ -z "${commit_failure_marker}" ] || [ -z "${commit_fingerprint}" ]; then
  exec "${REAL_GIT}" "$@"
fi

if "${REAL_GIT}" "$@"; then
  rm -f "${commit_failure_marker}"
  exit 0
else
  commit_status=$?
fi
post_fingerprint="$(commit_attempt_fingerprint "$@" || true)"
if [ "${post_fingerprint}" = "${commit_fingerprint}" ]; then
  marker_tmp="$(mktemp "${commit_failure_marker}.XXXXXX" 2>/dev/null || true)"
  if [ -n "${marker_tmp}" ] \
    && printf '%s\n' "${commit_fingerprint}" > "${marker_tmp}" \
    && mv -f "${marker_tmp}" "${commit_failure_marker}"; then
    echo "CSA git-guard: hook-enabled commit failed inside the filesystem sandbox; the staged tree is preserved." >&2
    echo "CSA git-guard: an identical retry is blocked. Inspect the hook failure, then use the same hook-enabled commit outside the sandbox if host resources are required." >&2
  else
    rm -f "${marker_tmp:-}"
  fi
else
  rm -f "${commit_failure_marker}"
fi
exit "${commit_status}"
