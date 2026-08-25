#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
source "$repo_root/scripts/tests/quality-gate-receipt-tests.sh"
receipt_contract_install_failure_trap quality-gate-receipt-intentional-local-tests.sh

run_intentional_local_artifacts_receipt() {
  local fixture counter runner output identity second manifest
  fixture="$(new_fixture)"
  runner="${fixture}/scripts/hooks/quality-gate-receipt.sh"
  counter="${fixture}/target/quality-gate-test-state/gate-counter"
  printf 'local-hooks\n' >"$fixture/.lefthook-local.yml"
  printf '{}\n' >"$fixture/hermes-tui-active-session-dllg3s3s.json"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  identity="$(printf '%s' "$output" | json_field receipt_identity)"
  assert_eq intentional-local-artifacts-status executed \
    "$(printf '%s' "$output" | json_field status)"
  assert_eq intentional-local-artifacts-reason receipt_missing \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq intentional-local-artifacts-gate-runs 1 "$(wc -c <"$counter")"
  assert_path_exists intentional-local-artifacts-receipt \
    "$fixture/.csa/state/quality-gate-receipts/${identity}.json"
  manifest="$(receipt_manifest "$fixture")"
  assert_contains intentional-local-artifacts-head \
    "head_oid=$(git -C "$fixture" rev-parse HEAD)" "$manifest"
  assert_contains intentional-local-artifacts-tree \
    "tree_oid=$(git -C "$fixture" rev-parse 'HEAD^{tree}')" "$manifest"
  assert_contains intentional-local-artifacts-empty-untracked \
    "untracked_worktree_digest=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
    "$manifest"
  second="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq intentional-local-artifacts-reuse reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq intentional-local-artifacts-reuse-runs 1 "$(wc -c <"$counter")"
  printf 'unrelated\n' >"$fixture/unrelated-untracked"
  output="$(cd "$fixture" && "$runner" -- scripts/hooks/fake-quality-gate.sh "$counter")"
  assert_eq intentional-local-unrelated-reason dirty_state \
    "$(printf '%s' "$output" | json_field rejection_reason)"
  assert_eq intentional-local-unrelated-runs 2 "$(wc -c <"$counter")"
  echo "PASS intentional-local-artifacts"
}

receipt_contract_set_case intentional-local-artifacts
run_intentional_local_artifacts_receipt
