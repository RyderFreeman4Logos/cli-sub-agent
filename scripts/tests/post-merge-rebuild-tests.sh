#!/usr/bin/env bash
# Ordering/failure regressions for scripts/hooks/post-merge-rebuild.sh (#2686 + cargo clean).
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
HOOK="$ROOT/scripts/hooks/post-merge-rebuild.sh"

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

# --- install success then cargo clean; full success only after both ---
test_install_then_clean_success() {
    local td bin_dir install_dir out order
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 0

    out="$(run_hook "$bin_dir" "$install_dir" 2>&1)"
    order="$(cat "$bin_dir/order.log")"

    assert_contains "$order" $'just install\ncargo clean' "order success"
    assert_contains "$out" "active-binary provenance verified" "success install msg"
    assert_contains "$out" "cargo clean completed" "success clean msg"
    assert_contains "$out" "Post-merge rebuild finished successfully" "full success"
    echo "ok: install then clean success"
}

# --- install failure must skip cargo clean and not claim full success ---
test_install_failure_skips_clean() {
    local td bin_dir install_dir out order
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 1 0

    out="$(run_hook "$bin_dir" "$install_dir" 2>&1 || true)"
    order="$(cat "$bin_dir/order.log")"

    assert_contains "$order" "just install" "install attempted"
    assert_not_contains "$order" "cargo clean" "clean skipped after install fail"
    assert_contains "$out" "just install failed" "install fail warning"
    assert_contains "$out" "skipping cargo clean" "skip clean note"
    assert_not_contains "$out" "Post-merge rebuild finished successfully" "no full success"
    echo "ok: install failure skips clean"
}

# --- clean failure after install must be explicit partial completion ---
test_clean_failure_is_partial() {
    local td bin_dir install_dir out order
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 1

    out="$(run_hook "$bin_dir" "$install_dir" 2>&1 || true)"
    order="$(cat "$bin_dir/order.log")"

    assert_contains "$order" $'just install\ncargo clean' "order partial"
    assert_contains "$out" "active-binary provenance verified" "install ok"
    assert_contains "$out" "cargo clean failed" "clean fail warning"
    assert_contains "$out" "partial" "partial completion"
    assert_not_contains "$out" "Post-merge rebuild finished successfully" "no full success"
    echo "ok: clean failure is partial"
}

# --- CSA session skip does not run install or clean ---
test_csa_session_skips() {
    local td bin_dir install_dir out order
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    setup_fakes "$bin_dir" 0 0

    out="$(
        env PATH="$bin_dir:$PATH" \
            CSA_SESSION_ID=test-session \
            CSA_POST_MERGE_INSTALL_DIR="$install_dir" \
            CSA_POST_MERGE_JUST="$bin_dir/just" \
            CSA_POST_MERGE_CARGO="$bin_dir/cargo" \
            bash "$HOOK" 2>&1
    )"
    order="$(cat "$bin_dir/order.log")"

    assert_contains "$out" "Inside CSA session" "session skip"
    assert_not_contains "$order" "just install" "no install in session"
    assert_not_contains "$order" "cargo clean" "no clean in session"
    echo "ok: CSA session skips rebuild"
}

# --- non-writable install dir skips rebuild ---
test_nonwritable_install_dir_skips() {
    local td bin_dir install_dir out order
    td="$(new_tmp_dir)"
    bin_dir="$td/bin"
    install_dir="$td/install"
    mkdir -p "$install_dir"
    chmod a-w "$install_dir"
    setup_fakes "$bin_dir" 0 0

    out="$(run_hook "$bin_dir" "$install_dir" 2>&1 || true)"
    order="$(cat "$bin_dir/order.log")"
    chmod u+w "$install_dir" || true

    assert_contains "$out" "not writable" "nonwritable skip"
    assert_not_contains "$order" "just install" "no install when nonwritable"
    assert_not_contains "$order" "cargo clean" "no clean when nonwritable"
    echo "ok: non-writable install dir skips"
}

test_install_then_clean_success
test_install_failure_skips_clean
test_clean_failure_is_partial
test_csa_session_skips
test_nonwritable_install_dir_skips

echo "All post-merge-rebuild tests passed."
