#!/usr/bin/env bash
# Ordering/failure regressions for scripts/hooks/post-merge-rebuild.sh (#2686 + cargo clean).
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
HOOK="$ROOT/scripts/hooks/post-merge-rebuild.sh"

# Parent-owned temp root: array mutation inside $(...) subshells is lost, so
# track fixtures under one root cleaned by EXIT.
TEST_ROOT="$(mktemp -d)"
cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

new_tmp_dir() {
    mktemp -d "$TEST_ROOT/case.XXXXXX"
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local label="$3"
    if [[ "$haystack" != *"$needle"* ]]; then
        echo "FAIL ($label): expected to contain: $needle" >&2
        echo "--- output ---" >&2
        printf '%s\n' "$haystack" >&2
        exit 1
    fi
}

assert_not_contains() {
    local haystack="$1"
    local needle="$2"
    local label="$3"
    if [[ "$haystack" == *"$needle"* ]]; then
        echo "FAIL ($label): did not expect: $needle" >&2
        echo "--- output ---" >&2
        printf '%s\n' "$haystack" >&2
        exit 1
    fi
}

assert_exit() {
    local actual="$1"
    local expected="$2"
    local label="$3"
    if [ "$actual" -ne "$expected" ]; then
        echo "FAIL ($label): expected exit $expected, got $actual" >&2
        exit 1
    fi
}

setup_fakes() {
    local bin_dir="$1"
    local install_rc="${2:-0}"
    local clean_rc="${3:-0}"
    mkdir -p "$bin_dir"
    cat >"$bin_dir/just" <<EOF
#!/usr/bin/env bash
set -euo pipefail
echo "just \$*" >>"$bin_dir/order.log"
if [ "\${1:-}" = "install" ]; then
    # Record install_dir=... so wiring regressions are visible.
    printf '%s\n' "\$*" >>"$bin_dir/just-args.log"
    exit $install_rc
fi
exit 0
EOF
    cat >"$bin_dir/cargo" <<EOF
#!/usr/bin/env bash
set -euo pipefail
echo "cargo \$*" >>"$bin_dir/order.log"
if [ "\${1:-}" = "clean" ]; then
    exit $clean_rc
fi
exit 0
EOF
    chmod +x "$bin_dir/just" "$bin_dir/cargo"
    : >"$bin_dir/order.log"
    : >"$bin_dir/just-args.log"
}

run_hook() {
    local bin_dir="$1"
    local install_dir="$2"
    shift 2
    env -u CSA_SESSION_ID \
        PATH="$bin_dir:$PATH" \
        CSA_POST_MERGE_INSTALL_DIR="$install_dir" \
        CSA_POST_MERGE_JUST="$bin_dir/just" \
        CSA_POST_MERGE_CARGO="$bin_dir/cargo" \
        bash "$HOOK" "$@"
}

# Capture stdout+stderr and exit status without tripping set -e.
run_hook_capture() {
    local bin_dir="$1"
    local install_dir="$2"
    local out_file="$3"
    set +e
    run_hook "$bin_dir" "$install_dir" >"$out_file" 2>&1
    local rc=$?
    set -e
    printf '%s\n' "$rc"
}

# --- install success then cargo clean; full success only after both ---
test_install_then_clean_success() {
    local td bin_dir install_dir out_file order rc out
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    out_file="$td/out.txt"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 0

    rc="$(run_hook_capture "$bin_dir" "$install_dir" "$out_file")"
    out="$(cat "$out_file")"
    order="$(cat "$bin_dir/order.log")"

    assert_exit "$rc" 0 "success exit"
    assert_contains "$order" $'just install install_dir='"$install_dir"$'\ncargo clean' "order success"
    assert_contains "$(cat "$bin_dir/just-args.log")" "install_dir=$install_dir" "install_dir wired"
    assert_contains "$out" "active-binary provenance verified" "success install msg"
    assert_contains "$out" "cargo clean completed" "success clean msg"
    assert_contains "$out" "Post-merge rebuild finished successfully" "full success"
    echo "ok: install then clean success"
}

# --- install failure must skip cargo clean, nonzero exit, no full success ---
test_install_failure_skips_clean() {
    local td bin_dir install_dir out_file order rc out
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    out_file="$td/out.txt"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 7 0

    rc="$(run_hook_capture "$bin_dir" "$install_dir" "$out_file")"
    out="$(cat "$out_file")"
    order="$(cat "$bin_dir/order.log")"

    assert_exit "$rc" 7 "install fail exit"
    assert_contains "$order" "just install" "install attempted"
    assert_not_contains "$order" "cargo clean" "clean skipped after install fail"
    assert_contains "$out" "just install failed" "install fail warning"
    assert_contains "$out" "skipping cargo clean" "skip clean note"
    assert_not_contains "$out" "Post-merge rebuild finished successfully" "no full success"
    echo "ok: install failure skips clean"
}

# --- clean failure after install: partial completion + nonzero ---
test_clean_failure_is_partial() {
    local td bin_dir install_dir out_file order rc out
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    out_file="$td/out.txt"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 3

    rc="$(run_hook_capture "$bin_dir" "$install_dir" "$out_file")"
    out="$(cat "$out_file")"
    order="$(cat "$bin_dir/order.log")"

    assert_exit "$rc" 3 "clean fail exit"
    assert_contains "$order" $'just install install_dir='"$install_dir"$'\ncargo clean' "order partial"
    assert_contains "$out" "active-binary provenance verified" "install ok"
    assert_contains "$out" "cargo clean failed" "clean fail warning"
    assert_contains "$out" "partial" "partial completion"
    assert_not_contains "$out" "Post-merge rebuild finished successfully" "no full success"
    echo "ok: clean failure is partial"
}

# --- CSA session skip does not run install or clean; exit 0 ---
test_csa_session_skips() {
    local td bin_dir install_dir out order rc
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 0

    set +e
    out="$(
        env PATH="$bin_dir:$PATH" \
            CSA_SESSION_ID=test-session \
            CSA_POST_MERGE_INSTALL_DIR="$install_dir" \
            CSA_POST_MERGE_JUST="$bin_dir/just" \
            CSA_POST_MERGE_CARGO="$bin_dir/cargo" \
            bash "$HOOK" 2>&1
    )"
    rc=$?
    set -e
    order="$(cat "$bin_dir/order.log")"

    assert_exit "$rc" 0 "session skip exit"
    assert_contains "$out" "Inside CSA session" "session skip"
    assert_not_contains "$order" "just install" "no install in session"
    assert_not_contains "$order" "cargo clean" "no clean in session"
    echo "ok: CSA session skips rebuild"
}

# --- non-writable install dir skips rebuild; exit 0 ---
test_nonwritable_install_dir_skips() {
    local td bin_dir install_dir out_file order rc out
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    out_file="$td/out.txt"
    mkdir -p "$install_dir"
    chmod a-w "$install_dir"
    setup_fakes "$bin_dir" 0 0

    rc="$(run_hook_capture "$bin_dir" "$install_dir" "$out_file")"
    out="$(cat "$out_file")"
    order="$(cat "$bin_dir/order.log")"
    chmod u+w "$install_dir" || true

    assert_exit "$rc" 0 "nonwritable skip exit"
    assert_contains "$out" "not writable" "nonwritable skip"
    assert_not_contains "$order" "just install" "no install when nonwritable"
    assert_not_contains "$order" "cargo clean" "no clean when nonwritable"
    echo "ok: non-writable install dir skips"
}

# --- fixture root is parent-owned (no leak of case dirs under /tmp after EXIT) ---
test_fixture_root_is_parent_owned() {
    if [ ! -d "$TEST_ROOT" ]; then
        echo "FAIL: TEST_ROOT missing" >&2
        exit 1
    fi
    local children
    children="$(find "$TEST_ROOT" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
    if [ "$children" -lt 1 ]; then
        echo "FAIL: expected case dirs under TEST_ROOT" >&2
        exit 1
    fi
    echo "ok: fixture root parent-owned ($children case dirs under $TEST_ROOT)"
}

test_install_then_clean_success
test_install_failure_skips_clean
test_clean_failure_is_partial
test_csa_session_skips
test_nonwritable_install_dir_skips
test_fixture_root_is_parent_owned

echo "All post-merge-rebuild tests passed."
