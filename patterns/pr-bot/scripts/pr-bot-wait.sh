#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: pr-bot-wait.sh <PR_NUMBER> [options]
  --timeout <SEC>         total wait budget
  --interval <SEC>        seconds between gh poll calls
  --bot-login <LOGIN>     bot GitHub login to match
  --output <FILE>         result JSON path
  --push-sha <SHA>        only accept reviews whose commit_id == this
  --quota-cache <FILE>    quota cache file
EOF
}

if [ "$#" -lt 1 ]; then
  usage
  exit 2
fi

PR_NUMBER="$1"
shift

TIMEOUT_SEC="${CSA_PR_BOT_TIMEOUT:-900}"
INTERVAL_SEC="${CSA_PR_BOT_INTERVAL:-30}"
BOT_LOGIN="${CSA_PR_BOT_LOGIN:-}"
OUTPUT_FILE="${CSA_PR_BOT_OUTPUT:-/tmp/pr-bot-wait-${PR_NUMBER}.json}"
PUSH_SHA=""
QUOTA_CACHE_FILE="${CSA_PR_BOT_QUOTA_CACHE:-${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/pr_review/cloud_bot_quota.toml}"
BOT_NAME="${CSA_PR_BOT_NAME:-}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --timeout)
      TIMEOUT_SEC="${2:?missing value for --timeout}"
      shift 2
      ;;
    --interval)
      INTERVAL_SEC="${2:?missing value for --interval}"
      shift 2
      ;;
    --bot-login)
      BOT_LOGIN="${2:?missing value for --bot-login}"
      shift 2
      ;;
    --output)
      OUTPUT_FILE="${2:?missing value for --output}"
      shift 2
      ;;
    --push-sha)
      PUSH_SHA="${2:?missing value for --push-sha}"
      shift 2
      ;;
    --quota-cache)
      QUOTA_CACHE_FILE="${2:?missing value for --quota-cache}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

case "${TIMEOUT_SEC}" in
  ''|*[!0-9]*)
    echo "timeout must be an integer" >&2
    exit 2
    ;;
esac

case "${INTERVAL_SEC}" in
  ''|*[!0-9]*)
    echo "interval must be an integer" >&2
    exit 2
    ;;
esac

if [ "${INTERVAL_SEC}" -le 0 ]; then
  echo "interval must be > 0" >&2
  exit 2
fi

if [ "${TIMEOUT_SEC}" -lt 0 ]; then
  echo "timeout must be >= 0" >&2
  exit 2
fi

repo_from_origin() {
  local origin_url
  local repo
  origin_url="$(git remote get-url origin 2>/dev/null || true)"
  if [ -z "${origin_url}" ]; then
    echo "ERROR: unable to determine GitHub repo from origin remote" >&2
    exit 2
  fi
  repo="$(
    printf '%s\n' "${origin_url}" | sed -nE \
    -e 's#^https?://([^@/]+@)?github\.com/([^/]+/[^/]+?)(\.git)?$#\2#p' \
    -e 's#^(ssh://)?([^@]+@)?github\.com[:/]([^/]+/[^/]+?)(\.git)?$#\3#p' \
    | head -n 1
  )"
  printf '%s\n' "${repo%.git}"
}

REPO="$(repo_from_origin)"
PUSH_TIME=""
if [ -n "${PUSH_SHA}" ]; then
  PUSH_TIME="$(git show -s --format=%cI "${PUSH_SHA}")"
fi

if [ -z "${BOT_NAME}" ]; then
  if [ -n "${BOT_LOGIN}" ]; then
    BOT_NAME="${BOT_LOGIN%\[bot\]}"
  else
    BOT_NAME="bot"
  fi
fi

write_output() {
  local json="$1"
  local tmp_file="${OUTPUT_FILE}.tmp"
  mkdir -p "$(dirname "${OUTPUT_FILE}")"
  printf '%s\n' "${json}" >"${tmp_file}"
  mv "${tmp_file}" "${OUTPUT_FILE}"
}

preview_body() {
  printf '%s' "$1" | tr '\r\n\t' '   ' | head -c 200
}

write_quota_cache() {
  local comment_body="$1"
  local quota_section_header="[cloud_bot_quota.${BOT_NAME}]"
  local quota_write_tmp="${QUOTA_CACHE_FILE}.tmp"
  local quota_body_tmp="${QUOTA_CACHE_FILE}.body.tmp"
  local quota_now_utc
  local quota_expected_reset_at
  local quota_short_message
  local quota_short_message_escaped

  quota_now_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  quota_expected_reset_at="$(date -u -d '+24 hours' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v+24H +%Y-%m-%dT%H:%M:%SZ)"
  quota_short_message="$(preview_body "${comment_body}")"
  quota_short_message_escaped="$(printf '%s' "${quota_short_message}" | sed 's/\\/\\\\/g; s/"/\\"/g')"

  mkdir -p "$(dirname "${QUOTA_CACHE_FILE}")"
  if [ -f "${QUOTA_CACHE_FILE}" ]; then
    awk -v target="${quota_section_header}" '
      $0 == target { skip = 1; next }
      skip && /^\[/ { skip = 0 }
      !skip { print }
    ' "${QUOTA_CACHE_FILE}" >"${quota_body_tmp}"
  else
    : >"${quota_body_tmp}"
  fi

  {
    cat "${quota_body_tmp}"
    if [ -s "${quota_body_tmp}" ]; then
      printf '\n'
    fi
    printf '%s\n' "${quota_section_header}"
    printf 'exhausted_at = "%s"\n' "${quota_now_utc}"
    printf 'expected_reset_at = "%s"\n' "${quota_expected_reset_at}"
    printf 'last_pr_seen = %s\n' "${PR_NUMBER}"
    printf 'last_quota_message = "%s"\n' "${quota_short_message_escaped}"
  } >"${quota_write_tmp}"
  rm -f "${quota_body_tmp}"
  mv "${quota_write_tmp}" "${QUOTA_CACHE_FILE}"

  QUOTA_EXHAUSTED_AT="${quota_now_utc}"
  QUOTA_EXPECTED_RESET_AT="${quota_expected_reset_at}"
}

review_filter() {
  if [ -n "${BOT_LOGIN}" ]; then
    jq -c --arg login "${BOT_LOGIN}" --arg push_time "${PUSH_TIME}" --arg push_sha "${PUSH_SHA}" '
      [ .[] | .[]
        | select((.user.login // "") == $login)
        | select($push_time == "" or (.submitted_at // "") > $push_time)
        | select($push_sha == "" or (.commit_id // "") == $push_sha)
      ]
      | sort_by(.submitted_at)
      | last // empty
    '
  else
    jq -c --arg push_time "${PUSH_TIME}" --arg push_sha "${PUSH_SHA}" '
      [ .[] | .[]
        | select((.user.type // "") == "Bot" or ((.user.login // "") | test("\\[bot\\]$")))
        | select($push_time == "" or (.submitted_at // "") > $push_time)
        | select($push_sha == "" or (.commit_id // "") == $push_sha)
      ]
      | sort_by(.submitted_at)
      | last // empty
    '
  fi
}

elapsed=0

while [ "${elapsed}" -le "${TIMEOUT_SEC}" ]; do
  reviews_json="$(gh api --paginate --slurp "repos/${REPO}/pulls/${PR_NUMBER}/reviews?per_page=100")"
  review_json="$(printf '%s\n' "${reviews_json}" | review_filter || true)"
  if [ -n "${review_json}" ]; then
    result_json="$(
      jq -n \
        --argjson pr "${PR_NUMBER}" \
        --argjson elapsed "${elapsed}" \
        --argjson review "${review_json}" \
        '{status:"replied", pr:$pr, elapsed_seconds:$elapsed, review:$review}'
    )"
    write_output "${result_json}"
    exit 0
  fi

  comments_json="$(gh api --paginate --slurp "repos/${REPO}/issues/${PR_NUMBER}/comments?per_page=100")"
  quota_comment_json="$(
    if [ -n "${BOT_LOGIN}" ]; then
      printf '%s\n' "${comments_json}" | jq -c --arg login "${BOT_LOGIN}" --arg push_time "${PUSH_TIME}" '
        [ .[] | .[]
          | select((.user.login // "") == $login)
          | select($push_time == "" or (.created_at // "") > $push_time)
          | select((.body // "") | test("daily quota limit"; "i"))
        ]
        | sort_by(.created_at)
        | last // empty
      ' || true
    else
      printf '%s\n' "${comments_json}" | jq -c --arg push_time "${PUSH_TIME}" '
        [ .[] | .[]
          | select((.user.type // "") == "Bot" or ((.user.login // "") | test("\\[bot\\]$")))
          | select($push_time == "" or (.created_at // "") > $push_time)
          | select((.body // "") | test("daily quota limit"; "i"))
        ]
        | sort_by(.created_at)
        | last // empty
      ' || true
    fi
  )"
  if [ -n "${quota_comment_json}" ]; then
    quota_body="$(printf '%s\n' "${quota_comment_json}" | jq -r '.body // ""')"
    quota_preview="$(preview_body "${quota_body}")"
    quota_created_at="$(printf '%s\n' "${quota_comment_json}" | jq -r '.created_at // ""')"
    write_quota_cache "${quota_body}"
    result_json="$(
      jq -n \
        --argjson pr "${PR_NUMBER}" \
        --argjson elapsed "${elapsed}" \
        --arg preview "${quota_preview}" \
        --arg created_at "${quota_created_at}" \
        --arg exhausted_at "${QUOTA_EXHAUSTED_AT}" \
        --arg expected_reset_at "${QUOTA_EXPECTED_RESET_AT}" \
        '{status:"quota_exhausted", pr:$pr, elapsed_seconds:$elapsed, exhausted_at:$exhausted_at, expected_reset_at:$expected_reset_at, comment:{preview:$preview, created_at:$created_at}}'
    )"
    write_output "${result_json}"
    exit 0
  fi

  reply_comment_json="$(
    if [ -n "${BOT_LOGIN}" ]; then
      printf '%s\n' "${comments_json}" | jq -c --arg login "${BOT_LOGIN}" --arg push_time "${PUSH_TIME}" '
        [ .[] | .[]
          | select((.user.login // "") == $login)
          | select($push_time == "" or (.created_at // "") > $push_time)
        ]
        | sort_by(.created_at)
        | last // empty
      ' || true
    else
      printf '%s\n' "${comments_json}" | jq -c --arg push_time "${PUSH_TIME}" '
        [ .[] | .[]
          | select((.user.type // "") == "Bot" or ((.user.login // "") | test("\\[bot\\]$")))
          | select($push_time == "" or (.created_at // "") > $push_time)
        ]
        | sort_by(.created_at)
        | last // empty
      ' || true
    fi
  )"
  if [ -n "${reply_comment_json}" ]; then
    reply_body="$(printf '%s\n' "${reply_comment_json}" | jq -r '.body // ""')"
    reply_preview="$(preview_body "${reply_body}")"
    reply_created_at="$(printf '%s\n' "${reply_comment_json}" | jq -r '.created_at // ""')"
    result_json="$(
      jq -n \
        --argjson pr "${PR_NUMBER}" \
        --argjson elapsed "${elapsed}" \
        --arg preview "${reply_preview}" \
        --arg created_at "${reply_created_at}" \
        '{status:"replied", pr:$pr, elapsed_seconds:$elapsed, comment:{preview:$preview, created_at:$created_at}}'
    )"
    write_output "${result_json}"
    exit 0
  fi

  if [ "${elapsed}" -ge "${TIMEOUT_SEC}" ]; then
    break
  fi

  sleep "${INTERVAL_SEC}"
  elapsed=$((elapsed + INTERVAL_SEC))
done

timeout_json="$(
  jq -n \
    --argjson pr "${PR_NUMBER}" \
    --argjson elapsed "${TIMEOUT_SEC}" \
    '{status:"timeout", pr:$pr, elapsed_seconds:$elapsed}'
)"
write_output "${timeout_json}"
exit 1
