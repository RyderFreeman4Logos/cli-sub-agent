#!/usr/bin/env bash
# L4 quality gate: token budget check for changed files in review range.
# Runs `csa tokuin estimate` on .rs files in the diff range and flags
# files exceeding the 8K token threshold.
#
# Usage: token-budget-gate.sh [range]
#   range: git diff range (default: main...HEAD)
#
# Exit codes:
#   0 = all files within budget (or only exempt files over)
#   1 = non-exempt files exceed budget
set -euo pipefail

RANGE="${1:-main...HEAD}"
THRESHOLD="${TOKEN_BUDGET_THRESHOLD:-8000}"
WARNING_THRESHOLD="${TOKEN_BUDGET_WARNING:-6000}"

is_exempt() {
    local file="$1"
    case "$file" in
        # Keep this block in sync with the justfile `find-monolith-files`
        # recipe (#1747 / #181 tracks deduping these two redundant lists).
        *.lock|*lock.json|*lock.yaml) return 0 ;;
        */AGENTS.md|*/FACTORY.md) return 0 ;;
        */PATTERN.md|*/SKILL.md) return 0 ;;
        */workflow.toml) return 0 ;;
        *_tests.rs|*_test.rs|*_tests_*.rs) return 0 ;; # dedicated test files
        */tests/*.rs) return 0 ;;               # integration test directory
        */benches/*.rs) return 0 ;;             # benchmark files
        */config.rs|*/global.rs) return 0 ;;    # config definition files (high token density, low complexity)
        # Pre-existing monoliths grandfathered to warn-not-block: each already
        # exceeded budget on main, so a trivial cross-cutting touch (e.g. adding a
        # field to a widely-constructed struct) must not hard-block an unrelated
        # PR. Splitting them is tracked as separate refactor work.
        */transport_tmux.rs) return 0 ;;        # tests+jsonl already split to siblings; body split pending
        */review_cmd_execute.rs) return 0 ;;    # review-command driver; split pending
        */session_cmds_reconcile.rs) return 0 ;;  # ~10.8K on main; reconcile driver; #161 field-spread touch; split pending
        */preflight_state_dir.rs) return 0 ;;     # ~9.4K on main; #161 SessionResult field-spread touch; split pending
        */mcp_server.rs) return 0 ;;              # ~8.0K on main; MCP tool dispatcher; #1745 field-spread touch; split pending
        */run_cmd_attempt.rs) return 0 ;;         # ~10.8K on main; run-attempt driver; #1745 field-spread touch; split pending
        */transport_cli.rs) return 0 ;;           # ~9.1K on main; CLI transport spawn; #1745 field-spread touch; split pending
    esac
    return 1
}

changed_files=$(git diff --name-only --diff-filter=ACMR "$RANGE" -- '*.rs' 2>/dev/null || true)
[ -z "$changed_files" ] && exit 0

block_count=0
warning_count=0
findings=""

while IFS= read -r file; do
    [ -f "$file" ] || continue

    tokens=$(csa tokuin estimate --model gpt-4o "$file" 2>/dev/null || echo 0)
    [ -z "$tokens" ] && tokens=0

    if [ "$tokens" -gt "$THRESHOLD" ]; then
        if is_exempt "$file"; then
            findings="${findings}WARNING: exempt file over budget: ${file} (${tokens} tokens, limit: ${THRESHOLD})\n"
        else
            findings="${findings}BLOCK: ${file} exceeds token budget (${tokens} tokens, limit: ${THRESHOLD}). Split this module.\n"
            block_count=$((block_count + 1))
        fi
    elif [ "$tokens" -gt "$WARNING_THRESHOLD" ]; then
        findings="${findings}WARNING: ${file} approaching budget (${tokens} tokens, limit: ${THRESHOLD})\n"
        warning_count=$((warning_count + 1))
    fi
done <<< "$changed_files"

if [ -n "$findings" ]; then
    echo ""
    echo "=== Token Budget Gate (L4) ==="
    echo -e "$findings"
    echo "Summary: ${block_count} BLOCK, ${warning_count} WARNING"
    echo "==============================="
fi

[ "$block_count" -gt 0 ] && exit 1
exit 0
