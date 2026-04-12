#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: gen_commit_msg.sh [--subject|--body] [scope]

Options:
  --subject   Output only the Conventional Commits subject line
  --body      Output only the commit body portion
EOF
}

component_for_file() {
  local file="$1"

  case "${file}" in
    crates/*)
      printf 'crate %s' "$(printf '%s' "${file}" | cut -d/ -f2)"
      ;;
    patterns/*)
      printf 'pattern %s' "$(printf '%s' "${file}" | cut -d/ -f2)"
      ;;
    scripts/*)
      printf 'scripts'
      ;;
    tests/*|*/tests/*|*_test.rs|*.spec.ts|*.test.ts)
      printf 'tests'
      ;;
    docs/*|drafts/*|*.md)
      printf 'docs'
      ;;
    Cargo.toml|Cargo.lock|weave.lock|*/Cargo.toml|*/Cargo.lock|*/weave.lock)
      printf 'workspace metadata'
      ;;
    */*)
      printf '%s' "${file%%/*}"
      ;;
    *)
      printf 'repo root files'
      ;;
  esac
}

collect_components() {
  local file component existing

  components=()
  for file in "${staged_files[@]}"; do
    component="$(component_for_file "${file}")"
    for existing in "${components[@]:-}"; do
      if [[ "${existing}" == "${component}" ]]; then
        component=""
        break
      fi
    done

    if [[ -n "${component}" ]]; then
      components+=("${component}")
    fi
  done
}

describe_components() {
  local total preview_count

  total="${#components[@]}"
  if [[ "${total}" -eq 0 ]]; then
    printf 'the staged files'
    return
  fi

  if [[ "${total}" -eq 1 ]]; then
    printf '%s' "${components[0]}"
    return
  fi

  if [[ "${total}" -eq 2 ]]; then
    printf '%s and %s' "${components[0]}" "${components[1]}"
    return
  fi

  preview_count=3
  if [[ "${total}" -lt "${preview_count}" ]]; then
    preview_count="${total}"
  fi

  printf '%s, %s, and %s' "${components[0]}" "${components[1]}" "${components[2]}"
  if [[ "${total}" -gt "${preview_count}" ]]; then
    printf ' (+%s more)' "$((total - preview_count))"
  fi
}

build_commit_body() {
  local summary targets

  collect_components
  targets="$(describe_components)"

  if [[ "${is_release}" == "true" ]]; then
    summary="Bump the staged release metadata touching ${targets}."
  elif [[ "${is_docs_only}" == "true" ]]; then
    summary="Refresh the staged documentation touching ${targets}."
  elif [[ "${is_tests_only}" == "true" ]]; then
    summary="Update the staged test coverage touching ${targets}."
  elif [[ "${has_new_non_test_code}" == "true" ]]; then
    summary="Add the staged functionality touching ${targets}."
  else
    summary="Update the staged changes touching ${targets}."
  fi

  cat <<EOF
${summary}

### AI Reviewer Metadata
- **Design Intent**: Capture the staged changes in a non-empty fallback commit body when no richer upstream summary was provided.
- **Key Decisions**: Derive this fallback from the staged file set and preserve the AI Reviewer Metadata scaffold required by the audited commit workflow.
- **Reviewer Guidance**: Verify the staged diff still matches this generated summary and expand the metadata when the change needs task-specific rationale.
EOF
}

mode="full"
scope_input=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --subject)
      if [[ "${mode}" != "full" ]]; then
        echo "ERROR: --subject and --body are mutually exclusive" >&2
        exit 1
      fi
      mode="subject"
      shift
      ;;
    --body)
      if [[ "${mode}" != "full" ]]; then
        echo "ERROR: --subject and --body are mutually exclusive" >&2
        exit 1
      fi
      mode="body"
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    -*)
      echo "ERROR: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      if [[ -n "${scope_input}" ]]; then
        echo "ERROR: only one optional scope argument is supported" >&2
        usage >&2
        exit 1
      fi
      scope_input="$1"
      shift
      ;;
  esac
done

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

commit_subject=""
if [[ "${is_release}" == "true" ]]; then
  commit_subject='chore(release): bump workspace and lockfiles'
elif [[ "${is_docs_only}" == "true" ]]; then
  commit_subject="$(printf 'docs(%s): update documentation' "${scope}")"
elif [[ "${is_tests_only}" == "true" ]]; then
  commit_subject="$(printf 'test(%s): update test coverage' "${scope}")"
elif [[ "${has_new_non_test_code}" == "true" ]]; then
  commit_subject="$(printf 'feat(%s): add staged functionality' "${scope}")"
else
  commit_subject="$(printf 'fix(%s): update staged changes' "${scope}")"
fi

commit_body=""
commit_body="$(build_commit_body)"

case "${mode}" in
  subject)
    printf '%s\n' "${commit_subject}"
    ;;
  body)
    if [[ -n "${commit_body}" ]]; then
      printf '%s\n' "${commit_body}"
    fi
    ;;
  full)
    printf '%s\n' "${commit_subject}"
    ;;
esac
