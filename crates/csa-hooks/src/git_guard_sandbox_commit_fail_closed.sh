FINGERPRINT_MAX_BYTES=8388608
FINGERPRINT_MAX_BLOCKS=16384
FINGERPRINT_MAX_FILES=64
FINGERPRINT_FILE_MAX_BYTES=1048576
FINGERPRINT_COMMAND_TIMEOUT=2
FINGERPRINT_FILE_TIMEOUT=1
MARKER_MAX_BYTES=1024

copy_bounded_regular_file() {
  copy_source="$1"
  copy_destination="$2"
  copy_max_bytes="$3"
  [ -f "${copy_source}" ] && [ ! -L "${copy_source}" ] || return 1
  copy_identity_before="$(/usr/bin/stat -c '%d:%i:%s:%f:%y:%z' -- "${copy_source}" 2>/dev/null)" || return 1
  copy_size_before="$(/usr/bin/stat -c '%s' -- "${copy_source}" 2>/dev/null)" || return 1
  case "${copy_size_before}" in ""|*[!0-9]*) return 1 ;; esac
  [ "${copy_size_before}" -le "${copy_max_bytes}" ] || return 1
  timeout "${FINGERPRINT_FILE_TIMEOUT}" dd if="${copy_source}" of="${copy_destination}" \
    iflag=nofollow,nonblock,count_bytes count="$((copy_max_bytes + 1))" status=none \
    2>/dev/null || return 1
  copy_size="$(/usr/bin/stat -c '%s' -- "${copy_destination}" 2>/dev/null)" || return 1
  case "${copy_size}" in ""|*[!0-9]*) return 1 ;; esac
  [ "${copy_size}" -le "${copy_max_bytes}" ] || return 1
  copy_identity_after="$(/usr/bin/stat -c '%d:%i:%s:%f:%y:%z' -- "${copy_source}" 2>/dev/null)" || return 1
  [ "${copy_identity_before}" = "${copy_identity_after}" ] || return 1
  [ "${copy_size}" = "${copy_size_before}" ] || return 1
}

read_bounded_single_record() {
  record_source="$1"
  record_max_bytes="$2"
  record_dir="$(mktemp -d "${session_dir}/.git-guard-record.XXXXXX" 2>/dev/null)" || return 1
  record_copy="${record_dir}/record"
  if ! copy_bounded_regular_file "${record_source}" "${record_copy}" "${record_max_bytes}"; then
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  fi
  record_size="$(/usr/bin/stat -c '%s' -- "${record_copy}" 2>/dev/null)" || {
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  }
  if [ "${record_size}" -eq 0 ]; then
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  fi
  record_lines="$(wc -l < "${record_copy}")" || {
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  }
  case "${record_lines}" in ""|*[!0-9]*) record_lines=0 ;; esac
  printf '\n' > "${record_dir}/newline" || return 1
  dd if="${record_copy}" of="${record_dir}/last" iflag=skip_bytes,count_bytes \
    skip="$((record_size - 1))" count=1 status=none 2>/dev/null || return 1
  if [ "${record_lines}" -ne 1 ] || ! cmp -s "${record_dir}/last" "${record_dir}/newline"; then
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  fi
  IFS= read -r record_value < "${record_copy}" || {
    /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
    return 1
  }
  /usr/bin/rm -rf -- "${record_dir}" 2>/dev/null || true
  printf '%s\n' "${record_value}"
}

bounded_capture_status() {
  capture_name="$1"
  shift
  capture_path="${fingerprint_tmp_dir}/${capture_name}"
  if (
    ulimit -f "${FINGERPRINT_MAX_BLOCKS}" || exit 1
    exec timeout "${FINGERPRINT_COMMAND_TIMEOUT}" "$@"
  ) > "${capture_path}" 2>/dev/null; then
    capture_status=0
  else
    capture_status=$?
  fi
  capture_size="$(/usr/bin/stat -c '%s' -- "${capture_path}" 2>/dev/null)" || return 1
  case "${capture_size}" in ""|*[!0-9]*) return 1 ;; esac
  [ "${capture_size}" -le "${FINGERPRINT_MAX_BYTES}" ]
}

bounded_capture() {
  bounded_capture_status "$@" || return 1
  [ "${capture_status}" -eq 0 ]
}

hash_input_file() {
  fingerprint_hash_serial=$((fingerprint_hash_serial + 1))
  hash_output="hash.${fingerprint_hash_serial}"
  bounded_capture "${hash_output}" "${REAL_GIT}" hash-object "$1" || return 1
  hash_value="$(read_bounded_single_record "${fingerprint_tmp_dir}/${hash_output}" 128)" || return 1
  case "${hash_value}" in *[!0-9a-f]*) return 1 ;; esac
  case "${#hash_value}" in 40|64) ;; *) return 1 ;; esac
  printf '%s\n' "${hash_value}"
}

append_hashed_file() {
  hashed_label="$1"
  hashed_path="$2"
  hashed_manifest="$3"
  has_control_bytes "${hashed_path}" && return 1
  fingerprint_file_count=$((fingerprint_file_count + 1))
  [ "${fingerprint_file_count}" -le "${FINGERPRINT_MAX_FILES}" ] || return 1
  hashed_copy="${fingerprint_tmp_dir}/file.${fingerprint_file_count}"
  copy_bounded_regular_file "${hashed_path}" "${hashed_copy}" \
    "${FINGERPRINT_FILE_MAX_BYTES}" || return 1
  hashed_size="$(/usr/bin/stat -c '%s' -- "${hashed_copy}" 2>/dev/null)" || return 1
  fingerprint_total_file_bytes=$((fingerprint_total_file_bytes + hashed_size))
  [ "${fingerprint_total_file_bytes}" -le "${FINGERPRINT_MAX_BYTES}" ] || return 1
  hashed_value="$(hash_input_file "${hashed_copy}")" || return 1
  printf '%s:%s:%s:%s\n' "${hashed_label}" "${#hashed_path}" "${hashed_path}" \
    "${hashed_value}" >> "${hashed_manifest}" || return 1
}

commit_attempt_fingerprint_inner() {
  LC_ALL=C
  export LC_ALL
  fingerprint_hash_serial=0
  fingerprint_file_count=0
  fingerprint_total_file_bytes=0

  bounded_capture_status head "${REAL_GIT}" rev-parse --verify HEAD || return 1
  case "${capture_status}" in
    0) head="$(read_bounded_single_record "${fingerprint_tmp_dir}/head" 128)" || return 1 ;;
    128) head=unborn ;;
    *) return 1 ;;
  esac
  bounded_capture index "${REAL_GIT}" write-tree || return 1
  index_tree="$(read_bounded_single_record "${fingerprint_tmp_dir}/index" 128)" || return 1

  (
    ulimit -f "${FINGERPRINT_MAX_BLOCKS}" || exit 1
    for arg do
      printf '%s:%s\n' "${#arg}" "${arg}" || exit 1
    done
  ) > "${fingerprint_tmp_dir}/args" || return 1
  args_size="$(/usr/bin/stat -c '%s' -- "${fingerprint_tmp_dir}/args" 2>/dev/null)" || return 1
  [ "${args_size}" -le "${FINGERPRINT_MAX_BYTES}" ] || return 1
  args_hash="$(hash_input_file "${fingerprint_tmp_dir}/args")" || return 1

  bounded_capture env.raw env || return 1
  bounded_capture env.sorted sort "${fingerprint_tmp_dir}/env.raw" || return 1
  env_hash="$(hash_input_file "${fingerprint_tmp_dir}/env.sorted")" || return 1

  bounded_capture config "${REAL_GIT}" config --null --list --show-origin || return 1
  config_hash="$(hash_input_file "${fingerprint_tmp_dir}/config")" || return 1

  if [ -n "${top}" ]; then
    bounded_capture worktree "${REAL_GIT}" -C "${top}" diff --no-ext-diff \
      --no-textconv --binary -- || return 1
    worktree_hash="$(hash_input_file "${fingerprint_tmp_dir}/worktree")" || return 1
  else
    worktree_hash=absent
  fi

  hooks_manifest="${fingerprint_tmp_dir}/hooks.manifest"
  : > "${hooks_manifest}" || return 1
  printf 'worktree:%s\n' "${worktree_hash}" >> "${hooks_manifest}" || return 1
  if [ -n "${lefthook_config:-}" ]; then
    append_hashed_file lefthook "${lefthook_config}" "${hooks_manifest}" || return 1
  else
    printf '%s\n' lefthook:absent >> "${hooks_manifest}" || return 1
  fi

  found_hook=false
  if [ -n "${top}" ]; then
    pre_commit_path="$(hook_path_for pre-commit)" || return 1
    [ -n "${pre_commit_path}" ] || return 1
    hooks_dir="$(dirname "${pre_commit_path}")" || return 1
    bounded_capture hooks.raw find -P "${hooks_dir}" -type f -print || return 1
    bounded_capture hooks.sorted sort "${fingerprint_tmp_dir}/hooks.raw" || return 1
    while IFS= read -r hook_path || [ -n "${hook_path}" ]; do
      [ -n "${hook_path}" ] || continue
      found_hook=true
      if [ -x "${hook_path}" ]; then hook_mode=x; else hook_mode=-; fi
      append_hashed_file "hook:${hook_mode}" "${hook_path}" "${hooks_manifest}" || return 1
    done < "${fingerprint_tmp_dir}/hooks.sorted"
  fi

  # The supported helper closure is every regular file below the active hooks
  # directory plus colon-delimited absolute files declared here. Tracked
  # worktree helpers are already covered by worktree_hash; untracked or
  # out-of-tree helpers must be declared explicitly.
  declared_helpers="${CSA_GIT_GUARD_HOOK_HELPERS:-}"
  if [ -n "${declared_helpers}" ]; then
    case ":${declared_helpers}:" in *::*) return 1 ;; esac
  fi
  while [ -n "${declared_helpers}" ]; do
    case "${declared_helpers}" in
      *:*) helper_path="${declared_helpers%%:*}"; declared_helpers="${declared_helpers#*:}" ;;
      *) helper_path="${declared_helpers}"; declared_helpers="" ;;
    esac
    case "${helper_path}" in /*) ;; *) return 1 ;; esac
    append_hashed_file helper "${helper_path}" "${hooks_manifest}" || return 1
  done
  [ "${found_hook}" = true ] || printf '%s\n' hooks:absent >> "${hooks_manifest}" || return 1
  hooks_hash="$(hash_input_file "${hooks_manifest}")" || return 1

  printf '%s %s %s %s %s %s\n' "${head}" "${index_tree}" "${args_hash}" \
    "${env_hash}" "${config_hash}" "${hooks_hash}"
}

commit_attempt_fingerprint() {
  fingerprint_tmp_dir="$(mktemp -d "${session_dir}/.git-guard-fingerprint.XXXXXX" 2>/dev/null)" || return 1
  commit_attempt_fingerprint_inner "$@"
  fingerprint_status=$?
  /usr/bin/rm -rf -- "${fingerprint_tmp_dir}" 2>/dev/null || true
  return "${fingerprint_status}"
}

write_commit_failure_marker() {
  marker_value="$1"
  marker_tmp="$(mktemp "${commit_failure_marker}.XXXXXX" 2>/dev/null)" || return 1
  if printf '%s\n' "${marker_value}" > "${marker_tmp}" \
    && mv -f "${marker_tmp}" "${commit_failure_marker}"; then
    return 0
  fi
  /usr/bin/rm -f -- "${marker_tmp}" 2>/dev/null || true
  return 1
}

block_unavailable_fingerprint() {
  echo "BLOCKED: sandbox commit fingerprint state is unavailable; mandatory hooks will not run on uncertain state." >&2
  echo "The staged tree is preserved. Inspect the fingerprint failure, then run the same hook-enabled commit outside the sandbox." >&2
  exit 1
}

commit_failure_marker="" commit_fingerprint=""
if [ "${CSA_FS_SANDBOXED:-}" = "1" ] && [ -n "${CSA_SESSION_DIR:-}" ]; then
  session_dir="$(canonical_directory "${CSA_SESSION_DIR}")" || session_dir=""
  guard_dir="$(canonical_directory "$(dirname "$0")")" || guard_dir=""
  expected_guard_dir="$(canonical_directory "${session_dir}/bin")" || expected_guard_dir=""
  if [ -n "${session_dir}" ] && [ "${guard_dir}" = "${expected_guard_dir}" ]; then
    commit_failure_marker="${session_dir}/__CSA_GIT_COMMIT_FAILURE_MARKER__"
    if ! commit_fingerprint="$(commit_attempt_fingerprint "$@")"; then
      write_commit_failure_marker "uncertain fingerprint producer failure" || true
      block_unavailable_fingerprint
    fi
    if [ -e "${commit_failure_marker}" ] || [ -L "${commit_failure_marker}" ]; then
      if blocked_fingerprint="$(read_bounded_single_record "${commit_failure_marker}" "${MARKER_MAX_BYTES}")"; then
        if [ "${blocked_fingerprint}" = "${commit_fingerprint}" ]; then
          echo "BLOCKED: hook-enabled commit already failed for this unchanged staged tree in the filesystem sandbox; mandatory hooks will not be rerun." >&2
          echo "The staged tree is preserved. Inspect the first hook failure, then run the same hook-enabled commit outside the sandbox if it needs host resources." >&2
          exit 1
        fi
      else
        write_commit_failure_marker "uncertain invalid fingerprint marker" || true
        block_unavailable_fingerprint
      fi
    fi
  fi
fi

if [ -z "${commit_failure_marker}" ]; then
  exec "${REAL_GIT}" "$@"
fi

if "${REAL_GIT}" "$@"; then
  /usr/bin/rm -f -- "${commit_failure_marker}" 2>/dev/null || true
  exit 0
else
  commit_status=$?
fi
if post_fingerprint="$(commit_attempt_fingerprint "$@")"; then
  if [ "${post_fingerprint}" = "${commit_fingerprint}" ]; then
    if write_commit_failure_marker "${commit_fingerprint}"; then
      echo "CSA git-guard: hook-enabled commit failed inside the filesystem sandbox; the staged tree is preserved." >&2
      echo "CSA git-guard: an identical retry is blocked. Inspect the hook failure, then use the same hook-enabled commit outside the sandbox if host resources are required." >&2
    fi
  else
    /usr/bin/rm -f -- "${commit_failure_marker}" 2>/dev/null || true
  fi
else
  write_commit_failure_marker "uncertain fingerprint producer failure" || true
  echo "CSA git-guard: post-hook fingerprint state is unavailable; the staged tree is preserved for host recovery." >&2
fi
exit "${commit_status}"
