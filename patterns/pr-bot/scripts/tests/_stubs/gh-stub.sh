#!/usr/bin/env bash

set -euo pipefail

state_dir="${GH_STUB_STATE_DIR:?}"
scenario="${GH_STUB_SCENARIO:?}"
expected_head="${GH_STUB_EXPECTED_HEAD:?}"
expected_base="${GH_STUB_EXPECTED_BASE:?}"

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
  json_arg="$(arg_value "--json" "$@")"
  if [ "${head_arg}" != "${expected_head}" ]; then
    echo "unexpected --head: ${head_arg}" >&2
    exit 1
  fi
  if [ "${base_arg}" != "${expected_base}" ]; then
    echo "unexpected --base: ${base_arg}" >&2
    exit 1
  fi
  if [ "${json_arg}" != "number,headRefName,headRepositoryOwner" ]; then
    echo "unexpected --json: ${json_arg}" >&2
    exit 1
  fi

  list_call="$(count_call pr-list)"
  case "${scenario}" in
    create-success)
      if [ "${list_call}" -ge 2 ]; then
        printf '101\n'
      fi
      ;;
    preexisting)
      printf '202\n'
      ;;
    missed-already-exists)
      if [ "${list_call}" -ge 2 ]; then
        printf '303\n'
      fi
      ;;
    ambiguous)
      printf '404\n405\n'
      ;;
    *)
      echo "unknown GH_STUB_SCENARIO: ${scenario}" >&2
      exit 1
      ;;
  esac
  exit 0
fi

if [ "${1:-}" = "pr" ] && [ "${2:-}" = "create" ]; then
  head_arg="$(arg_value "--head" "$@")"
  base_arg="$(arg_value "--base" "$@")"
  if [ "${head_arg}" != "${expected_head}" ]; then
    echo "unexpected create --head: ${head_arg}" >&2
    exit 1
  fi
  if [ "${base_arg}" != "${expected_base}" ]; then
    echo "unexpected create --base: ${base_arg}" >&2
    exit 1
  fi
  count_call pr-create >/dev/null
  case "${scenario}" in
    create-success)
      printf 'https://github.com/test-owner/test-repo/pull/101\n'
      ;;
    missed-already-exists)
      echo "a pull request for branch \"test-owner:fix/1171\" into branch \"main\" already exists" >&2
      exit 1
      ;;
    preexisting|ambiguous)
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
