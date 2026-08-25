#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -lt 1 ]; then
  echo "usage: session-wait-until-done.sh <session-id> [--cd <path>]" >&2
  exit 2
fi

session_id="$1"
shift

model_provider="${CSA_MODEL_PROVIDER:-}"
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
