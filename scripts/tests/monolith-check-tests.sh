#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
CHECKER="$ROOT/scripts/monolith/check.sh"

tmp_dirs=()
cleanup() {
    if [ "${#tmp_dirs[@]}" -gt 0 ]; then
        rm -rf "${tmp_dirs[@]}"
    fi
}
trap cleanup EXIT

new_tmp_dir() {
    local dir
    dir="$(mktemp -d)"
    tmp_dirs+=("$dir")
    printf '%s\n' "$dir"
}

write_fake_csa() {
    local bin_dir="$1"
    mkdir -p "$bin_dir"
    cat >"$bin_dir/csa" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [ "${CSA_FAKE_FAIL:-0}" = "1" ]; then
    echo "fake csa unavailable" >&2
    exit 127
fi

if [ "${CSA_FAKE_BAD_JSON:-0}" = "1" ]; then
    echo "not-json"
    exit 0
fi

file="${@: -1}"
case "$file" in
    *grown.rs) tokens=21 ;;
    *under.rs) tokens=9 ;;
    *) tokens=20 ;;
esac

printf '{"tokens":%s}\n' "$tokens"
SH
    chmod +x "$bin_dir/csa"
}

setup_repo() {
    local repo="$1"
    mkdir -p "$repo"
    git -C "$repo" init -q
    git -C "$repo" config user.email "test@example.invalid"
    git -C "$repo" config user.name "Monolith Test"
}

run_checker() {
    local repo="$1"
    local baseline="$2"
    shift 2
    (
        cd "$repo"
        MONOLITH_TOKEN_THRESHOLD=10 \
            MONOLITH_LINE_THRESHOLD=3 \
            TOKUIN_MODEL=gpt-4o \
            "$CHECKER" "$@" --baseline "$baseline" --report-all
    )
}

assert_failure() {
    local name="$1"
    local output
    shift
    if output="$("$@" 2>&1)"; then
        echo "FAIL: expected failure: $name" >&2
        printf '%s\n' "$output" >&2
        exit 1
    fi
}

test_new_over_budget_file_hard_fails() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/new_over.rs"
    baseline="$repo/baseline.toml"
    printf '' >"$baseline"
    git -C "$repo" add src/new_over.rs
    git -C "$repo" commit -q -m init

    PATH="$bin_dir:$PATH" assert_failure "new over-budget file hard-fails" \
        run_checker "$repo" "$baseline" --scope all
}

test_baselined_file_within_cap_passes_with_warning() {
    local repo bin_dir baseline output
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/base.rs"
    baseline="$repo/baseline.toml"
    cat >"$baseline" <<'EOF'
[[files]]
path = "src/base.rs"
kind = "source"
baseline_tokens = 20
baseline_lines = 1
issue = "1880"
rationale = "test"
EOF
    git -C "$repo" add src/base.rs
    git -C "$repo" commit -q -m init

    output="$(PATH="$bin_dir:$PATH" run_checker "$repo" "$baseline" --scope all)"
    grep -q 'WARNING baseline debt: src/base.rs' <<<"$output"
}

test_baselined_file_growth_hard_fails() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/grown.rs"
    baseline="$repo/baseline.toml"
    cat >"$baseline" <<'EOF'
[[files]]
path = "src/grown.rs"
kind = "source"
baseline_tokens = 20
baseline_lines = 1
issue = "1880"
rationale = "test"
EOF
    git -C "$repo" add src/grown.rs
    git -C "$repo" commit -q -m init

    PATH="$bin_dir:$PATH" assert_failure "baselined growth hard-fails" \
        run_checker "$repo" "$baseline" --scope all
}

test_empty_issue_hard_fails() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/base.rs"
    baseline="$repo/baseline.toml"
    cat >"$baseline" <<'EOF'
[[files]]
path = "src/base.rs"
kind = "source"
baseline_tokens = 20
baseline_lines = 1
issue = ""
rationale = "test"
EOF
    git -C "$repo" add src/base.rs
    git -C "$repo" commit -q -m init

    PATH="$bin_dir:$PATH" assert_failure "empty baseline issue hard-fails" \
        run_checker "$repo" "$baseline" --scope all
}

test_report_all_lists_multiple_offenders() {
    local repo bin_dir baseline output
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn first() {}\n' >"$repo/src/first.rs"
    printf 'fn second() {}\n' >"$repo/src/second.rs"
    baseline="$repo/baseline.toml"
    printf '' >"$baseline"
    git -C "$repo" add src/first.rs src/second.rs
    git -C "$repo" commit -q -m init

    set +e
    output="$(PATH="$bin_dir:$PATH" run_checker "$repo" "$baseline" --scope all 2>&1)"
    status=$?
    set -e
    [ "$status" -ne 0 ]
    grep -q 'src/first.rs' <<<"$output"
    grep -q 'src/second.rs' <<<"$output"
}

test_tokenizer_unavailable_fails_closed() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/new_over.rs"
    baseline="$repo/baseline.toml"
    printf '' >"$baseline"
    git -C "$repo" add src/new_over.rs
    git -C "$repo" commit -q -m init

    CSA_FAKE_FAIL=1 PATH="$bin_dir:$PATH" assert_failure "tokenizer unavailable fails closed" \
        run_checker "$repo" "$baseline" --scope all
}

test_tokenizer_unparsable_fails_closed() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf 'fn main() {}\n' >"$repo/src/new_over.rs"
    baseline="$repo/baseline.toml"
    printf '' >"$baseline"
    git -C "$repo" add src/new_over.rs
    git -C "$repo" commit -q -m init

    CSA_FAKE_BAD_JSON=1 PATH="$bin_dir:$PATH" assert_failure "tokenizer unparsable output fails closed" \
        run_checker "$repo" "$baseline" --scope all
}

test_new_over_budget_test_file_hard_fails() {
    local repo bin_dir baseline
    repo="$(new_tmp_dir)"
    bin_dir="$(new_tmp_dir)"
    setup_repo "$repo"
    write_fake_csa "$bin_dir"
    mkdir -p "$repo/src"
    printf '#[test]\nfn it_works() {}\n' >"$repo/src/new_tests.rs"
    baseline="$repo/baseline.toml"
    printf '' >"$baseline"
    git -C "$repo" add src/new_tests.rs
    git -C "$repo" commit -q -m init

    PATH="$bin_dir:$PATH" assert_failure "new over-budget test file hard-fails" \
        run_checker "$repo" "$baseline" --scope all
}

test_new_over_budget_file_hard_fails
test_baselined_file_within_cap_passes_with_warning
test_baselined_file_growth_hard_fails
test_empty_issue_hard_fails
test_report_all_lists_multiple_offenders
test_tokenizer_unavailable_fails_closed
test_tokenizer_unparsable_fails_closed
test_new_over_budget_test_file_hard_fails

echo "monolith-check-tests: PASS"
