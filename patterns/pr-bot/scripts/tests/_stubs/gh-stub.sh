#!/usr/bin/env bash

set -euo pipefail

state_dir="${GH_STUB_STATE_DIR:?}"
scenario="${GH_STUB_SCENARIO:?}"
expected_list_head="${GH_STUB_EXPECTED_LIST_HEAD:?}"
expected_create_head="${GH_STUB_EXPECTED_CREATE_HEAD:?}"
expected_base="${GH_STUB_EXPECTED_BASE:?}"
expected_title="${GH_STUB_EXPECTED_TITLE:-}"

count_call() {
  local name="$1"
  local file="${state_dir}/${name}-count"
  local count=0
  if [ -f "${file}" ]; then
    count="$(<"${file}")"
  fi
  count=$((count + 1))
  printf '%s\n' "${count}" >"${file}"
  printf '%s\n' "${count}"
}

arg_value() {
  local flag="$1"
  shift
  while [ "$#" -gt 0 ]; do
    if [ "${1}" = "${flag}" ]; then
      shift
      printf '%s\n' "${1:-}"
      return 0
    fi
    shift
  done
  return 1
}

if [ "${1:-}" = "pr" ] && [ "${2:-}" = "list" ]; then
  head_arg="$(arg_value "--head" "$@")"
  base_arg="$(arg_value "--base" "$@")"
  state_arg="$(arg_value "--state" "$@")"
  json_arg="$(arg_value "--json" "$@")"
  if [ "${head_arg}" != "${expected_list_head}" ]; then
    echo "unexpected --head: ${head_arg}" >&2
    exit 1
  fi
  if [ "${base_arg}" != "${expected_base}" ]; then
    echo "unexpected --base: ${base_arg}" >&2
    exit 1
  fi
  if [ "${state_arg}" != "all" ]; then
    echo "unexpected --state: ${state_arg}" >&2
    exit 1
  fi
  if [ "${json_arg}" != "number,baseRefName,headRefName,headRepositoryOwner,state,mergedAt" ]; then
    echo "unexpected --json: ${json_arg}" >&2
    exit 1
  fi

  list_call="$(count_call pr-list)"
  case "${scenario}" in
    create-success)
      if [ "${list_call}" -ge 2 ]; then
        printf '[{"number":101,"baseRefName":"main","headRefName":"%s","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n' "${expected_list_head}"
      else
        printf '[]\n'
      fi
      ;;
    preexisting)
      printf '[{"number":202,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      ;;
    missed-already-exists)
      if [ "${list_call}" -ge 2 ]; then
        printf '[{"number":303,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      else
        printf '[]\n'
      fi
      ;;
    stale-already-exists)
      if [ "${list_call}" -ge 3 ]; then
        printf '[{"number":808,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      else
        printf '[]\n'
      fi
      ;;
    ambiguous)
      printf '[{"number":404,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null},{"number":405,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      ;;
    cross-owner)
      if [ "${list_call}" -ge 2 ]; then
        printf '[{"number":101,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      else
        printf '[{"number":606,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"other-owner"},"state":"OPEN","mergedAt":null}]\n'
      fi
      ;;
    quoted-branch)
      printf '[{"number":707,"baseRefName":"main","headRefName":"feat/has\\"quote","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}]\n'
      ;;
    merged)
      printf '[{"number":909,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"MERGED","mergedAt":"2026-05-22T21:19:08Z"}]\n'
      ;;
    closed)
      printf '[{"number":410,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"CLOSED","mergedAt":null}]\n'
      ;;
    *)
      echo "unknown GH_STUB_SCENARIO: ${scenario}" >&2
      exit 1
      ;;
  esac
  exit 0
fi

if [ "${1:-}" = "pr" ] && [ "${2:-}" = "view" ]; then
  branch_arg="${3:-}"
  json_arg="$(arg_value "--json" "$@")"
  if [ "${branch_arg}" != "${expected_list_head}" ]; then
    echo "unexpected pr view branch: ${branch_arg}" >&2
    exit 1
  fi
  if [ "${json_arg}" != "number,baseRefName,headRefName,headRepositoryOwner,state,mergedAt" ]; then
    echo "unexpected pr view --json: ${json_arg}" >&2
    exit 1
  fi

  view_call="$(count_call pr-view)"
  case "${scenario}" in
    preexisting)
      printf '{"number":202,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}\n'
      ;;
    merged)
      printf '{"number":909,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"MERGED","mergedAt":"2026-05-22T21:19:08Z"}\n'
      ;;
    closed)
      printf '{"number":410,"baseRefName":"main","headRefName":"fix/1171","headRepositoryOwner":{"login":"test-owner"},"state":"CLOSED","mergedAt":null}\n'
      ;;
    create-success)
      if [ -f "${state_dir}/pr-create-count" ] && [ "${view_call}" -ge 2 ]; then
        printf '{"number":101,"baseRefName":"main","headRefName":"%s","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}\n' "${expected_list_head}"
      else
        exit 1
      fi
      ;;
    quoted-branch)
      printf '{"number":707,"baseRefName":"main","headRefName":"feat/has\\"quote","headRepositoryOwner":{"login":"test-owner"},"state":"OPEN","mergedAt":null}\n'
      ;;
    *)
      exit 1
      ;;
  esac
  exit 0
fi

if [ "${1:-}" = "pr" ] && [ "${2:-}" = "create" ]; then
  head_arg="$(arg_value "--head" "$@")"
  base_arg="$(arg_value "--base" "$@")"
  title_arg="$(arg_value "--title" "$@")"
  if [ "${head_arg}" != "${expected_create_head}" ]; then
    echo "unexpected create --head: ${head_arg}" >&2
    exit 1
  fi
  if [ "${base_arg}" != "${expected_base}" ]; then
    echo "unexpected create --base: ${base_arg}" >&2
    exit 1
  fi
  if [ -n "${expected_title}" ] && [ "${title_arg}" != "${expected_title}" ]; then
    echo "unexpected create --title: ${title_arg}" >&2
    exit 1
  fi
  count_call pr-create >/dev/null
  case "${scenario}" in
    create-success|cross-owner)
      printf 'https://github.com/test-owner/test-repo/pull/101\n'
      ;;
    missed-already-exists|stale-already-exists)
      echo "a pull request for branch \"test-owner:fix/1171\" into branch \"main\" already exists" >&2
      exit 1
      ;;
    preexisting|ambiguous|quoted-branch|merged|closed)
      echo "gh pr create should not be called for scenario ${scenario}" >&2
      exit 1
      ;;
    *)
      echo "unknown GH_STUB_SCENARIO: ${scenario}" >&2
      exit 1
      ;;
  esac
  exit 0
fi

if [ "${1:-}" = "repo" ] && [ "${2:-}" = "view" ]; then
  printf 'test-owner\n'
  exit 0
fi

echo "unexpected gh invocation: $*" >&2
exit 1
