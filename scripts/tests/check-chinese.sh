#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECKER="${ROOT_DIR}/scripts/check-chinese.sh"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

mkdir -p "${TMP_DIR}/output" "${TMP_DIR}/src"

printf '\346\261\211\n' > "${TMP_DIR}/output/spec.toml"
if (cd "${TMP_DIR}" && "${CHECKER}"); then
  echo "expected generated output Han characters to be ignored" >&2
  exit 1
fi

printf '\346\261\211\n' > "${TMP_DIR}/src/main.rs"
if ! (cd "${TMP_DIR}" && "${CHECKER}"); then
  echo "expected source Han characters to be detected" >&2
  exit 1
fi
