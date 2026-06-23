#!/usr/bin/env bash
set -euo pipefail

current_head="${1:?missing current head}"
default_branch="${2:?missing default branch}"
artifact_root="${3:-.csa/native-review-bypass}"

case "${artifact_root}" in
  *review-bypass.log)
    echo "Native review bypass refuses .csa/review-bypass.log; it is an audit log, not review evidence." >&2
    exit 1
    ;;
esac

case "${artifact_root}" in
  *.toml)
    artifact_path="${artifact_root}"
    ;;
  *)
    artifact_path="${artifact_root%/}/${current_head}.toml"
    ;;
esac

if [ ! -f "${artifact_path}" ]; then
  if [ -f ".csa/review-bypass.log" ]; then
    echo "Native review bypass ignored .csa/review-bypass.log because it is audit-only; run csa review or provide trusted native-review artifact ${artifact_path}." >&2
  fi
  exit 1
fi

artifact_size="$(wc -c <"${artifact_path}" | tr -d ' ')"
[ "${artifact_size}" -le 2048 ] || exit 1

artifact_value() {
  local key="$1"
  awk -v key="${key}" '
    /^[[:space:]]*(#|$)/ { next }
    {
      line = $0
      sub(/^[[:space:]]*/, "", line)
      if (line !~ ("^" key "[[:space:]]*=")) {
        next
      }
      sub("^" key "[[:space:]]*=[[:space:]]*", "", line)
      sub(/[[:space:]]+#.*$/, "", line)
      sub(/[[:space:]]+$/, "", line)
      if (line ~ /^".*"$/) {
        line = substr(line, 2, length(line) - 2)
      }
      print line
      exit
    }
  ' "${artifact_path}"
}

lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

schema_version="$(artifact_value schema_version)"
artifact_kind="$(lower "$(artifact_value artifact_kind)")"
source="$(lower "$(artifact_value source)")"
head_sha="$(lower "$(artifact_value head_sha)")"
range="$(lower "$(artifact_value range)")"
verdict="$(lower "$(artifact_value verdict)")"
branch_lower="$(lower "${default_branch}")"
head_lower="$(lower "${current_head}")"

[ "${schema_version}" = "1" ] || exit 1
[ "${artifact_kind}" = "native_review_bypass" ] || exit 1
[ "${source}" = "native" ] || exit 1
[ "${head_sha}" = "${head_lower}" ] || exit 1

if [ "${range}" != "${branch_lower}...head" ] && [ "${range}" != "${branch_lower}...${head_lower}" ]; then
  exit 1
fi

case "${verdict}" in
  clean|pass)
    ;;
  *)
    exit 1
    ;;
esac

printf 'artifact=%s source=native range=%s verdict=%s head_sha=%s\n' \
  "${artifact_path}" "${range}" "${verdict}" "${head_lower}"
