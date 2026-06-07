#!/usr/bin/env bash
# Pre-commit: path-included src modules compile under the integration test crate root.
set -euo pipefail

repo_root="$(realpath "$(git rev-parse --show-toplevel)")"
cd "$repo_root"

declare -a queue_files=()
declare -a queue_tests=()
declare -a violations=()
seen_entries=""
resolved_path_attr=""
path_attr_violation=""
self_test_sandbox=""

relative_path() {
    local path="$1"
    path="${path#$repo_root/}"
    printf '%s' "$path"
}

resolve_path_attr() {
    local including_file="$1"
    local path_attr="$2"
    local base_dir
    local resolved_path
    local canonical_path
    local including_rel

    resolved_path_attr=""
    path_attr_violation=""

    if [[ "$path_attr" = /* ]]; then
        resolved_path="$path_attr"
    else
        base_dir="$(dirname "$including_file")"
        resolved_path="$base_dir/$path_attr"
    fi

    if [ ! -e "$resolved_path" ]; then
        return
    fi

    if ! canonical_path="$(realpath "$resolved_path" 2>/dev/null)"; then
        return
    fi

    case "$canonical_path" in
        "$repo_root"/*)
            if [ -f "$canonical_path" ]; then
                resolved_path_attr="$canonical_path"
            fi
            ;;
        *)
            including_rel="$(relative_path "$including_file")"
            path_attr_violation="file $canonical_path is referenced by #[path = \"$path_attr\"] from $including_rel but resolves outside repository root"
            ;;
    esac
}

enqueue_file() {
    local file="$1"
    local origin_test="$2"
    local key

    key="$file	$origin_test"
    if printf '%s' "$seen_entries" | grep -Fqx "$key"; then
        return
    fi

    seen_entries="${seen_entries}${key}"$'\n'
    queue_files+=("$file")
    queue_tests+=("$origin_test")
}

extract_path_attr() {
    sed -E 's/.*#\[path[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/'
}

enqueue_test_src_includes() {
    local test_file="$1"
    local line=""
    local path_attr=""
    local src_file=""

    while IFS=: read -r _line line; do
        path_attr="$(printf '%s\n' "$line" | extract_path_attr)"
        resolve_path_attr "$test_file" "$path_attr"
        if [ -n "$path_attr_violation" ]; then
            violations+=("$path_attr_violation")
            continue
        fi
        src_file="$resolved_path_attr"
        if [ -z "$src_file" ]; then
            continue
        fi
        enqueue_file "$src_file" "$test_file"
    done < <(grep -nE '#\[path[[:space:]]*=[[:space:]]*"\.\./src/[^"]+"' "$test_file" || true)
}

enqueue_transitive_includes() {
    local src_file="$1"
    local origin_test="$2"
    local line=""
    local path_attr=""
    local child_file=""

    while IFS=: read -r _line line; do
        path_attr="$(printf '%s\n' "$line" | extract_path_attr)"
        resolve_path_attr "$src_file" "$path_attr"
        if [ -n "$path_attr_violation" ]; then
            violations+=("$path_attr_violation")
            continue
        fi
        child_file="$resolved_path_attr"
        if [ -z "$child_file" ]; then
            continue
        fi
        enqueue_file "$child_file" "$origin_test"
    done < <(grep -nE '#\[path[[:space:]]*=[[:space:]]*"[^"]+"' "$src_file" || true)
}

check_no_crate_root_references() {
    local src_file="$1"
    local origin_test="$2"
    local crate_refs=""
    local src_rel=""
    local test_rel=""

    src_rel="$(relative_path "$src_file")"
    test_rel="$(relative_path "$origin_test")"

    if [ ! -f "$src_file" ]; then
        return
    fi

    crate_refs="$(
        awk '
            index($0, "crate::") && $0 !~ /^[[:space:]]*\/\// {
                print FNR ":" $0
            }
        ' "$src_file"
    )"

    if [ -n "$crate_refs" ]; then
        violations+=("file $src_rel is #[path]-included by test $test_rel but contains \`crate::\` references which will fail in the test crate context"$'\n'"$crate_refs")
    fi
}

assert_eq() {
    local expected="$1"
    local actual="$2"
    local label="$3"

    if [ "$expected" != "$actual" ]; then
        printf 'self-test failed: %s\nexpected: %s\nactual:   %s\n' "$label" "$expected" "$actual" >&2
        exit 1
    fi
}

assert_empty() {
    local actual="$1"
    local label="$2"

    if [ -n "$actual" ]; then
        printf 'self-test failed: %s\nexpected empty, got: %s\n' "$label" "$actual" >&2
        exit 1
    fi
}

assert_violation_count() {
    local expected="$1"
    local label="$2"

    if [ "${#violations[@]}" -ne "$expected" ]; then
        printf 'self-test failed: %s\nexpected %s violation(s), got %s\n' "$label" "$expected" "${#violations[@]}" >&2
        printf '%s\n' "${violations[@]}" >&2
        exit 1
    fi
}

append_path_attr_violation() {
    if [ -n "$path_attr_violation" ]; then
        violations+=("$path_attr_violation")
    fi
}

run_self_tests() {
    local original_repo_root="$repo_root"
    local sandbox=""
    local resolved=""

    sandbox="$(mktemp -d "${TMPDIR:-/tmp}/check-path-included-src.XXXXXX")"
    self_test_sandbox="$sandbox"
    trap 'rm -rf "$self_test_sandbox"' EXIT

    mkdir -p "$sandbox/repo/a/b" "$sandbox/repo/crates/cli-sub-agent/src" "$sandbox/repo/crates/cli-sub-agent/tests" "$sandbox/etc"
    printf 'pub struct Cli;\n' > "$sandbox/repo/crates/cli-sub-agent/src/cli.rs"
    printf 'root:x:0:0:root:/root:/bin/sh\ncrate::outside\n' > "$sandbox/etc/passwd"

    repo_root="$(realpath "$sandbox/repo")"

    violations=()
    resolve_path_attr "$repo_root/a/b/test.rs" "../../../etc/passwd"
    append_path_attr_violation
    resolved="$resolved_path_attr"
    assert_empty "$resolved" "relative traversal outside repo is not resolved"
    assert_violation_count 1 "relative traversal outside repo is reported"

    violations=()
    resolve_path_attr "$repo_root/a/b/test.rs" "/etc/passwd"
    append_path_attr_violation
    resolved="$resolved_path_attr"
    assert_empty "$resolved" "absolute path outside repo is not resolved"
    assert_violation_count 1 "absolute path outside repo is reported"

    violations=()
    resolve_path_attr "$repo_root/crates/cli-sub-agent/tests/e2e.rs" "../src/cli.rs"
    append_path_attr_violation
    resolved="$resolved_path_attr"
    assert_eq "$repo_root/crates/cli-sub-agent/src/cli.rs" "$resolved" "relative path inside repo is allowed"
    assert_violation_count 0 "relative path inside repo has no violation"

    violations=()
    resolve_path_attr "$repo_root/a/b/test.rs" "../src/missing.rs"
    append_path_attr_violation
    resolved="$resolved_path_attr"
    assert_empty "$resolved" "missing target is skipped"
    assert_violation_count 0 "missing target has no violation"

    printf '#[path = "../src/../../../../etc/passwd"]\nmod passwd;\n' > "$repo_root/crates/cli-sub-agent/tests/e2e.rs"
    queue_files=()
    queue_tests=()
    seen_entries=""
    violations=()
    enqueue_test_src_includes "$repo_root/crates/cli-sub-agent/tests/e2e.rs"
    assert_violation_count 1 "test include traversal is reported"
    assert_eq "0" "${#queue_files[@]}" "test include traversal is not enqueued"

    printf '#[path = "../../../etc/passwd"]\nmod passwd;\n' > "$repo_root/a/b/source.rs"
    queue_files=()
    queue_tests=()
    seen_entries=""
    violations=()
    enqueue_transitive_includes "$repo_root/a/b/source.rs" "$repo_root/a/b/test.rs"
    assert_violation_count 1 "transitive traversal is reported"
    assert_eq "0" "${#queue_files[@]}" "transitive traversal is not enqueued"

    repo_root="$original_repo_root"
    rm -rf "$self_test_sandbox"
    self_test_sandbox=""
    trap - EXIT
}

if [ "${1:-}" = "--self-test" ]; then
    run_self_tests
    exit 0
fi

while IFS= read -r test_file; do
    enqueue_test_src_includes "$test_file"
done < <(
    {
        find crates -path '*/tests/*.rs' -type f -print 2>/dev/null || true
        find tests -maxdepth 1 -name '*.rs' -type f -print 2>/dev/null || true
    } | sort -u
)

index=0
while [ "$index" -lt "${#queue_files[@]}" ]; do
    src_file="${queue_files[$index]}"
    origin_test="${queue_tests[$index]}"

    check_no_crate_root_references "$src_file" "$origin_test"
    if [ -f "$src_file" ]; then
        enqueue_transitive_includes "$src_file" "$origin_test"
    fi

    index=$((index + 1))
done

if [ "${#violations[@]}" -gt 0 ]; then
    printf 'ERROR: #[path]-included src modules must be test-crate compatible.\n\n' >&2
    printf '%s\n\n' "${violations[@]}" >&2
    exit 1
fi
