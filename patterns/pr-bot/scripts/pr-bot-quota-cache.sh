#!/usr/bin/env bash

quota_cache_preview_body() {
  printf '%s' "$1" | tr '\r\n\t' '   ' | head -c 200
}

compute_quota_expected_reset_at() {
  date -u -d '+24 hours' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v+24H +%Y-%m-%dT%H:%M:%SZ
}

quota_comment_indicates_exhaustion() {
  local comment_body="${1:-}"
  local bot_login="${2:-${BOT_LOGIN:-${CLOUD_BOT_LOGIN:-}}}"
  local bot_name="${3:-${BOT_NAME:-${CLOUD_BOT_NAME:-${CSA_PR_BOT_NAME:-}}}}"

  if [ -z "${comment_body}" ]; then
    return 1
  fi

  if printf '%s\n' "${comment_body}" | grep -Eqi 'daily quota limit|resource exhausted|quota|exhausted|rate.{0,3}limit.{0,20}exceed'; then
    return 0
  fi

  if {
    [ "${bot_name}" = "gemini-code-assist" ] \
      || [ "${bot_login}" = "gemini-code-assist" ] \
      || [ "${bot_login}" = "gemini-code-assist[bot]" ];
  } && printf '%s\n' "${comment_body}" | grep -qi 'try again later'; then
    return 0
  fi

  return 1
}

write_quota_cache() {
  local exhausted_at="$1"
  local expected_reset_at="$2"
  local reason_kind="$3"
  local comment_body="${4:-}"
  local quota_cache_file="${QUOTA_CACHE_FILE:-${CLOUD_BOT_QUOTA_CACHE_FILE:-${CSA_PR_BOT_QUOTA_CACHE:-${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/pr_review/cloud_bot_quota.toml}}}"
  local bot_name="${BOT_NAME:-${CLOUD_BOT_NAME:-${CSA_PR_BOT_NAME:-bot}}}"
  local pr_number="${PR_NUMBER:-${PR_NUM:-}}"
  local quota_section_header="[cloud_bot_quota.${bot_name}]"
  local quota_write_tmp="${quota_cache_file}.tmp"
  local quota_body_tmp="${quota_cache_file}.body.tmp"
  local quota_short_message
  local quota_short_message_escaped

  if [ -z "${exhausted_at}" ] || [ -z "${expected_reset_at}" ] || [ -z "${reason_kind}" ]; then
    echo "write_quota_cache requires exhausted_at, expected_reset_at, and reason_kind" >&2
    return 2
  fi

  if [ -z "${pr_number}" ]; then
    echo "write_quota_cache requires PR_NUMBER or PR_NUM" >&2
    return 2
  fi

  quota_short_message="$(quota_cache_preview_body "${comment_body}")"
  quota_short_message_escaped="$(printf '%s' "${quota_short_message}" | sed 's/\\/\\\\/g; s/"/\\"/g')"

  mkdir -p "$(dirname "${quota_cache_file}")"
  if [ -f "${quota_cache_file}" ]; then
    awk -v target="${quota_section_header}" '
      $0 == target { skip = 1; next }
      skip && /^\[/ { skip = 0 }
      !skip { print }
    ' "${quota_cache_file}" >"${quota_body_tmp}"
  else
    : >"${quota_body_tmp}"
  fi

  {
    cat "${quota_body_tmp}"
    if [ -s "${quota_body_tmp}" ]; then
      printf '\n'
    fi
    printf '%s\n' "${quota_section_header}"
    printf 'exhausted_at = "%s"\n' "${exhausted_at}"
    printf 'expected_reset_at = "%s"\n' "${expected_reset_at}"
    printf 'last_pr_seen = %s\n' "${pr_number}"
    printf 'last_quota_message = "%s"\n' "${quota_short_message_escaped}"
  } >"${quota_write_tmp}"
  rm -f "${quota_body_tmp}"
  mv "${quota_write_tmp}" "${quota_cache_file}"

  export QUOTA_EXHAUSTED_AT="${exhausted_at}"
  export QUOTA_EXPECTED_RESET_AT="${expected_reset_at}"
  export CLOUD_BOT_QUOTA_EXHAUSTED_AT="${exhausted_at}"
  export CLOUD_BOT_QUOTA_EXPECTED_RESET_AT="${expected_reset_at}"
  export MERGE_WITHOUT_BOT_REASON_KIND="${reason_kind}"
}
