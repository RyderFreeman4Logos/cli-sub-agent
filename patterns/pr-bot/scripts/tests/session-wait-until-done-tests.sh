#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/csa/session-wait-until-done.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

cat >"${TMP_ROOT}/csa" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

count_file="${TEST_WAIT_COUNT_FILE:?}"
count=0
if [ -f "${count_file}" ]; then
  count="$(cat "${count_file}")"
fi
count=$((count + 1))
printf '%s' "${count}" >"${count_file}"

if [ "${count}" -eq 1 ]; then
  echo '<!-- CSA:SESSION_WAIT_KV_WARM session=test-session status=alive elapsed=0s action=re-wait -->'
else
  echo "terminal result"
fi
EOF
chmod +x "${TMP_ROOT}/csa"

output="$(
  PATH="${TMP_ROOT}:${PATH}" \
    TEST_WAIT_COUNT_FILE="${TMP_ROOT}/wait-count" \
    CSA_MODEL_PROVIDER="openai" \
    bash "${SCRIPT_PATH}" "test-session"
)"

if [ "$(cat "${TMP_ROOT}/wait-count")" -ne 2 ]; then
  echo "expected the helper to wait again after a KV-warm result" >&2
  exit 1
fi
if [[ "${output}" != *"CSA:SESSION_WAIT_KV_WARM"* ]] || [[ "${output}" != *"terminal result"* ]]; then
  echo "expected live and terminal wait output, got: ${output}" >&2
  exit 1
fi

echo "session-wait-until-done tests: PASS"
