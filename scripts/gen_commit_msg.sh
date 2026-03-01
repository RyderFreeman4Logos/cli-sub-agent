#!/usr/bin/env bash
set -euo pipefail

scope_input="${1:-}"

if ! git diff --cached --name-only | grep -q .; then
  echo "ERROR: no staged changes to generate a commit message" >&2
  exit 1
fi

mapfile -t staged_files < <(git diff --cached --name-only)
mapfile -t staged_status < <(git diff --cached --name-status)

is_release=true
is_docs_only=true
is_tests_only=true
has_new_non_test_code=false

for file in "${staged_files[@]}"; do
  case "${file}" in
    Cargo.toml|Cargo.lock|weave.lock|*/Cargo.toml|*/Cargo.lock|*/weave.lock) ;;
    *) is_release=false ;;
  esac

  case "${file}" in
    docs/*|drafts/*|*.md) ;;
    *) is_docs_only=false ;;
  esac

  case "${file}" in
    tests/*|*/tests/*|*_test.rs|*.spec.ts|*.test.ts) ;;
    *) is_tests_only=false ;;
  esac
done

for status_line in "${staged_status[@]}"; do
  status="$(printf '%s' "${status_line}" | awk '{print $1}')"
  file="$(printf '%s' "${status_line}" | awk '{print $2}')"
  if [[ "${status}" == A* ]]; then
    case "${file}" in
      docs/*|drafts/*|tests/*|*/tests/*|*_test.rs|*.spec.ts|*.test.ts|*.md|Cargo.toml|Cargo.lock|weave.lock|*/Cargo.toml|*/Cargo.lock|*/weave.lock)
        ;;
      *)
        has_new_non_test_code=true
        ;;
    esac
  fi
done

scope=""
if [[ -n "${scope_input}" ]]; then
  scope="$(printf '%s' "${scope_input}" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9._-' '-' | sed 's/^-*//;s/-*$//')"
fi

if [[ -z "${scope}" ]]; then
  first_file="${staged_files[0]}"
  case "${first_file}" in
    crates/*) scope="$(printf '%s' "${first_file}" | cut -d/ -f2)" ;;
    patterns/*) scope="workflow" ;;
    docs/*|*.md|drafts/*) scope="docs" ;;
    *) scope="core" ;;
  esac
fi

if [[ "${is_release}" == "true" ]]; then
  printf 'chore(release): bump workspace and lockfiles\n'
  exit 0
fi

if [[ "${is_docs_only}" == "true" ]]; then
  printf 'docs(%s): update documentation\n' "${scope}"
  exit 0
fi

if [[ "${is_tests_only}" == "true" ]]; then
  printf 'test(%s): update test coverage\n' "${scope}"
  exit 0
fi

if [[ "${has_new_non_test_code}" == "true" ]]; then
  printf 'feat(%s): add staged functionality\n' "${scope}"
  exit 0
fi

printf 'fix(%s): update staged changes\n' "${scope}"
