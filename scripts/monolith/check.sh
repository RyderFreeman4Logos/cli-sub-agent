#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
Usage: scripts/monolith/check.sh --scope all|staged|range [--range <git-range>] --baseline <path> [--report-all]

Scopes:
  all      all tracked text files
  staged   staged text files only
  range    text files changed in --range <git-range>
EOF
}

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 2
}

scope=""
range=""
baseline=""
report_all=false

while [ "$#" -gt 0 ]; do
    case "$1" in
        --scope)
            [ "$#" -ge 2 ] || die "--scope requires a value"
            scope="$2"
            shift 2
            ;;
        --range)
            [ "$#" -ge 2 ] || die "--range requires a value"
            range="$2"
            shift 2
            ;;
        --baseline)
            [ "$#" -ge 2 ] || die "--baseline requires a value"
            baseline="$2"
            shift 2
            ;;
        --report-all)
            report_all=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            die "unknown argument: $1"
            ;;
    esac
done

case "$scope" in
    all|staged|range) ;;
    "") usage; die "--scope is required" ;;
    *) usage; die "--scope must be one of: all, staged, range" ;;
esac

[ -n "$baseline" ] || die "--baseline is required"
if [ "$scope" = "range" ] && [ -z "$range" ]; then
    die "--scope range requires --range <git-range>"
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if [[ "$baseline" != /* ]]; then
    baseline="$repo_root/$baseline"
fi
[ -f "$baseline" ] || die "baseline file not found: $baseline"

token_threshold="${MONOLITH_TOKEN_THRESHOLD:-${TOKEN_BUDGET_THRESHOLD:-8000}}"
line_threshold="${MONOLITH_LINE_THRESHOLD:-800}"
model="${TOKUIN_MODEL:-gpt-4o}"

case "$token_threshold" in
    ''|*[!0-9]*) die "MONOLITH_TOKEN_THRESHOLD/TOKEN_BUDGET_THRESHOLD must be a positive integer" ;;
esac
case "$line_threshold" in
    ''|*[!0-9]*) die "MONOLITH_LINE_THRESHOLD must be a positive integer" ;;
esac
if [ "$token_threshold" -le 0 ] || [ "$line_threshold" -le 0 ]; then
    die "monolith thresholds must be positive integers"
fi

if [ -x "$repo_root/target/debug/csa" ]; then
    csa_bin="$repo_root/target/debug/csa"
elif command -v csa >/dev/null 2>&1; then
    csa_bin="$(command -v csa)"
else
    die "tokenizer unavailable: expected target/debug/csa or csa in PATH"
fi

tmp_files=()
cleanup() {
    if [ "${#tmp_files[@]}" -gt 0 ]; then
        rm -f "${tmp_files[@]}"
    fi
}
trap cleanup EXIT

new_tmp() {
    local path
    path="$(mktemp)"
    tmp_files+=("$path")
    printf '%s\n' "$path"
}

baseline_tsv="$(new_tmp)"
if ! python3 - "$baseline" >"$baseline_tsv" <<'PY'
import sys

try:
    import tomllib
except ModuleNotFoundError:
    print("ERROR: python3 tomllib is required to parse the baseline", file=sys.stderr)
    sys.exit(2)

baseline_path = sys.argv[1]

try:
    with open(baseline_path, "rb") as fh:
        data = tomllib.load(fh)
except Exception as exc:
    print(f"ERROR: failed to parse baseline TOML {baseline_path}: {exc}", file=sys.stderr)
    sys.exit(2)

entries = data.get("files", [])
if entries is None:
    entries = []
if not isinstance(entries, list):
    print("ERROR: baseline key 'files' must be an array of tables", file=sys.stderr)
    sys.exit(2)

seen = set()
for index, entry in enumerate(entries, start=1):
    if not isinstance(entry, dict):
        print(f"ERROR: baseline entry #{index} must be a table", file=sys.stderr)
        sys.exit(2)

    path = entry.get("path")
    kind = entry.get("kind")
    tokens = entry.get("baseline_tokens")
    lines = entry.get("baseline_lines")
    issue = entry.get("issue", "")

    if not isinstance(path, str) or not path:
        print(f"ERROR: baseline entry #{index} has missing/invalid path", file=sys.stderr)
        sys.exit(2)
    if "\t" in path or "\n" in path:
        print(f"ERROR: baseline entry path contains unsupported control whitespace: {path!r}", file=sys.stderr)
        sys.exit(2)
    if path in seen:
        print(f"ERROR: duplicate baseline entry for {path}", file=sys.stderr)
        sys.exit(2)
    seen.add(path)

    if kind not in {"source", "test", "doc", "config", "other"}:
        print(f"ERROR: baseline entry for {path} has invalid kind: {kind!r}", file=sys.stderr)
        sys.exit(2)
    if not isinstance(tokens, int) or tokens < 0:
        print(f"ERROR: baseline entry for {path} has invalid baseline_tokens", file=sys.stderr)
        sys.exit(2)
    if not isinstance(lines, int) or lines < 0:
        print(f"ERROR: baseline entry for {path} has invalid baseline_lines", file=sys.stderr)
        sys.exit(2)
    if issue is None:
        issue = ""
    if not isinstance(issue, str):
        print(f"ERROR: baseline entry for {path} has non-string issue", file=sys.stderr)
        sys.exit(2)

    print(f"{path}\t{kind}\t{tokens}\t{lines}\t{issue}")
PY
then
    exit 2
fi

declare -A baseline_kind=()
declare -A baseline_tokens=()
declare -A baseline_lines=()
declare -A baseline_issue=()
declare -a metadata_failures=()

while IFS=$'\t' read -r path kind tokens lines issue; do
    [ -n "$path" ] || continue
    baseline_kind["$path"]="$kind"
    baseline_tokens["$path"]="$tokens"
    baseline_lines["$path"]="$lines"
    baseline_issue["$path"]="$issue"
    if [ -z "$issue" ]; then
        metadata_failures+=("BLOCK baseline metadata: $path has empty issue")
    fi
done <"$baseline_tsv"

is_exempt_path() {
    local file="$1"
    case "$file" in
        *.lock|*lock.json|*lock.yaml) return 0 ;;
        .test-target/*|.test-target/**) return 0 ;;
        scripts/monolith/check.sh) return 0 ;;
        scripts/monolith/baseline.toml) return 0 ;;
        scripts/tests/monolith-check-tests.sh) return 0 ;;
    esac
    return 1
}

classify_kind() {
    local file="$1"
    case "$file" in
        *_tests.rs|*_test.rs|*_tests_*.rs|tests/*.rs|*/tests/*.rs|*/benches/*.rs)
            printf 'test\n'
            ;;
        *.md|*.markdown|*.mdx|*.txt|*.rst)
            printf 'doc\n'
            ;;
        *.toml|*.yml|*.yaml|*.json|*.jsonc|*.ini|*.cfg|*.ron)
            printf 'config\n'
            ;;
        *.rs|*.sh|*.bash|*.zsh|*.py|*.ts|*.tsx|*.js|*.jsx|*.go|*.proto|*.c|*.h|*.cpp|*.hpp|*.sql|*.nix|Dockerfile|Makefile|justfile)
            printf 'source\n'
            ;;
        *)
            printf 'other\n'
            ;;
    esac
}

line_count() {
    local file="$1"
    local lines
    if ! lines="$(wc -l <"$file" 2>/dev/null)"; then
        die "failed to count lines for $file"
    fi
    lines="$(printf '%s' "$lines" | tr -d '[:space:]')"
    case "$lines" in
        ''|*[!0-9]*) die "unparsable line count for $file: $lines" ;;
    esac
    printf '%s\n' "$lines"
}

byte_count() {
    local file="$1"
    local bytes
    if ! bytes="$(wc -c <"$file" 2>/dev/null)"; then
        die "failed to count bytes for $file"
    fi
    bytes="$(printf '%s' "$bytes" | tr -d '[:space:]')"
    case "$bytes" in
        ''|*[!0-9]*) die "unparsable byte count for $file: $bytes" ;;
    esac
    printf '%s\n' "$bytes"
}

token_count() {
    local file="$1"
    local output
    local stderr_file
    stderr_file="$(new_tmp)"

    if ! output="$("$csa_bin" tokuin estimate --model "$model" --json "$file" 2>"$stderr_file")"; then
        printf 'ERROR: tokenizer failed for %s using %s tokuin estimate --model %s --json\n' "$file" "$csa_bin" "$model" >&2
        if [ -s "$stderr_file" ]; then
            sed 's/^/  /' "$stderr_file" >&2
        fi
        exit 2
    fi

    if ! TOKEN_OUTPUT="$output" python3 - <<'PY'
import json
import os
import sys

raw = os.environ.get("TOKEN_OUTPUT", "")
try:
    data = json.loads(raw)
except Exception as exc:
    print(f"unparsable tokenizer JSON: {exc}", file=sys.stderr)
    sys.exit(2)

tokens = data.get("tokens", data.get("total"))
if not isinstance(tokens, int) or tokens < 0:
    print("tokenizer JSON did not contain a non-negative integer tokens/total field", file=sys.stderr)
    sys.exit(2)

print(tokens)
PY
    then
        printf 'ERROR: tokenizer output was unparsable for %s\n' "$file" >&2
        printf '%s\n' "$output" >&2
        exit 2
    fi
}

file_list="$(new_tmp)"
case "$scope" in
    all)
        git ls-files -z --recurse-submodules >"$file_list"
        ;;
    staged)
        git diff --cached --name-only -z --diff-filter=ACMR >"$file_list"
        ;;
    range)
        git diff --name-only -z --diff-filter=ACMR "$range" >"$file_list"
        ;;
esac

declare -a hard_failures=()
declare -a warnings=()
checked=0

for failure in "${metadata_failures[@]}"; do
    hard_failures+=("$failure")
done

while IFS= read -r -d '' file; do
    [ -f "$file" ] || continue
    if is_exempt_path "$file"; then
        continue
    fi
    grep -Iq '' "$file" 2>/dev/null || continue

    checked=$((checked + 1))
    lines="$(line_count "$file")"
    bytes="$(byte_count "$file")"
    tokens=0
    if [ -n "${baseline_tokens[$file]+set}" ] || [ "$lines" -gt "$line_threshold" ] || [ "$bytes" -gt "$token_threshold" ]; then
        tokens="$(token_count "$file")"
    fi
    kind="$(classify_kind "$file")"

    over_limit=false
    limit_reason=""
    if [ "$lines" -gt "$line_threshold" ]; then
        over_limit=true
        limit_reason="${limit_reason}${lines} lines > ${line_threshold}; "
    fi
    if [ "$tokens" -gt "$token_threshold" ]; then
        over_limit=true
        limit_reason="${limit_reason}${tokens} tokens > ${token_threshold}; "
    fi
    limit_reason="${limit_reason%; }"

    if [ -n "${baseline_tokens[$file]+set}" ]; then
        cap_tokens="${baseline_tokens[$file]}"
        cap_lines="${baseline_lines[$file]}"
        issue="${baseline_issue[$file]}"
        if [ "$tokens" -gt "$cap_tokens" ] || [ "$lines" -gt "$cap_lines" ]; then
            hard_failures+=("BLOCK ratchet: $file ($kind) grew to ${tokens} tokens/${lines} lines; baseline cap ${cap_tokens} tokens/${cap_lines} lines; issue #${issue}")
        else
            warnings+=("WARNING baseline debt: $file ($kind) is within cap at ${tokens} tokens/${lines} lines; cap ${cap_tokens} tokens/${cap_lines} lines; issue #${issue}")
        fi
    elif [ "$over_limit" = true ]; then
        hard_failures+=("BLOCK new monolith: $file ($kind) exceeds limit: ${limit_reason}")
    fi
done <"$file_list"

printf '=== Monolith Token Gate ===\n'
printf 'Scope: %s\n' "$scope"
if [ "$scope" = "range" ]; then
    printf 'Range: %s\n' "$range"
fi
printf 'Thresholds: %s lines, %s tokens (model: %s)\n' "$line_threshold" "$token_threshold" "$model"
printf 'Baseline: %s\n' "${baseline#"$repo_root"/}"
printf 'Checked text files: %s\n' "$checked"

if [ "${#warnings[@]}" -gt 0 ] && [ "$report_all" = true ]; then
    printf '\nWarnings:\n'
    printf '%s\n' "${warnings[@]}"
fi

if [ "${#hard_failures[@]}" -gt 0 ]; then
    printf '\nHard failures:\n'
    printf '%s\n' "${hard_failures[@]}"
fi

printf '\nSummary: %s hard failure(s), %s warning(s)\n' "${#hard_failures[@]}" "${#warnings[@]}"

if [ "${#hard_failures[@]}" -gt 0 ]; then
    exit 1
fi
exit 0
