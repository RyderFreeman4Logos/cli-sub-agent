#!/usr/bin/env bash

set -euo pipefail

project_root="${CSA_PROJECT_ROOT:-$(git rev-parse --show-toplevel)}"
branch="${1:-$(git -C "${project_root}" branch --show-current)}"
state_base="${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent"
project_key="${project_root#/}"
session_root="${state_base}/${project_key}/sessions"

if [ ! -d "${session_root}" ]; then
  exit 0
fi

while IFS= read -r session_id; do
  [ -n "${session_id}" ] || continue
  review_meta_path="${session_root}/${session_id}/review_meta.json"
  [ -f "${review_meta_path}" ] || continue

  head_sha="$(
    jq -er '
      select(.decision == "pass")
      | select(.scope | startswith("base:") or startswith("range:"))
      | .head_sha
    ' "${review_meta_path}" 2>/dev/null || true
  )"
  if [ -n "${head_sha}" ]; then
    printf '%s\n' "${head_sha}"
    exit 0
  fi
done < <(
  csa session list --branch "${branch}" --format json 2>/dev/null \
    | jq -r '
        sort_by(.last_accessed) | reverse | .[]
        | select((.task_context.task_type // "") == "review" or ((.description // "") | startswith("review:")))
        | .session_id
      ' 2>/dev/null || true
)

exit 0
