#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: session-wait-until-done.sh <session-id> [--cd <path>]" >&2
  exit 2
fi

session_id="$1"
shift

normalize_model_provider_key() {
  local raw="${1:-}"
  raw="${raw#\"}"
  raw="${raw%\"}"
  raw="${raw#\'}"
  raw="${raw%\'}"
  raw="$(printf '%s' "${raw}" | tr '[:upper:]' '[:lower:]')"
  raw="${raw#"${raw%%[![:space:]]*}"}"
  raw="${raw%"${raw##*[![:space:]]}"}"
  printf '%s' "${raw}"
}

normalize_hermes_model_provider() {
  local provider
  provider="$(normalize_model_provider_key "${1:-}")"
  case "${provider}" in
    anthropic|claude) printf 'claude' ;;
    openai|openai-codex) printf 'openai' ;;
    zai|zhipu|zhipuai|glm) printf 'glm' ;;
    xai|xai-oauth|grok) printf 'xai' ;;
    *) printf '%s' "${provider}" ;;
  esac
}

hermes_config_provider() {
  local path="${HOME}/.hermes/config.yaml"
  [ -f "${path}" ] || return 0
  awk '
    /^[[:space:]]*(#|$)/ { next }
    {
      line = $0
      indent = match(line, /[^[:space:]]/) - 1
      if (indent < 0) indent = 0
      trimmed = line
      sub(/^[[:space:]]+/, "", trimmed)
      if (model_indent != "" && indent <= model_indent + 0) model_indent = ""
      if (match(trimmed, /^model\.provider:[[:space:]]*/)) {
        val = substr(trimmed, RSTART + RLENGTH)
        sub(/[[:space:]]+#.*$/, "", val)
        gsub(/["'\'']/, "", val)
        print val
        exit
      }
      if (trimmed ~ /^model:[[:space:]]*$/) {
        model_indent = indent
        next
      }
      if (model_indent != "" && match(trimmed, /^provider:[[:space:]]*/)) {
        val = substr(trimmed, RSTART + RLENGTH)
        sub(/[[:space:]]+#.*$/, "", val)
        gsub(/["'\'']/, "", val)
        print val
        exit
      }
    }
  ' "${path}"
}

model_provider="$(normalize_model_provider_key "${CSA_MODEL_PROVIDER:-}")"
if [ -z "${model_provider}" ] && [ "${CSA_CALLER_TOOL:-}" = "hermes" ]; then
  model_provider="$(normalize_hermes_model_provider "${HERMES_MODEL_PROVIDER:-}")"
  if [ -z "${model_provider}" ]; then
    model_provider="$(normalize_hermes_model_provider "$(hermes_config_provider)")"
  fi
fi
if [ -z "${model_provider}" ]; then
  echo "ERROR: CSA_MODEL_PROVIDER is required; run pr-bot from a supported parent agent or pass --var CSA_MODEL_PROVIDER=<configured provider>" >&2
  exit 2
fi

wait_args=(--session "${session_id}" --model-provider "${model_provider}")
if [ "$#" -gt 0 ]; then
  wait_args+=("$@")
fi

while true; do
  set +e
  wait_output="$(csa session wait "${wait_args[@]}" 2>&1)"
  wait_rc=$?
  set -e

  if [ -n "${wait_output}" ]; then
    printf '%s\n' "${wait_output}"
  fi

  if [ "${wait_rc}" -eq 124 ]; then
    echo "INFO: session ${session_id} is still running after one wait window; retrying." >&2
    continue
  fi

  exit "${wait_rc}"
done
