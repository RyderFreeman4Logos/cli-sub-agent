#!/usr/bin/env bash
# Ordering/failure regressions for scripts/hooks/post-merge-rebuild.sh (#2686 + cargo clean).
# Also validates real Just install argument semantics (positional vs keyword).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$ROOT/scripts/hooks/post-merge-rebuild.sh"

pass=0
fail=0

assert_eq() {
    local got="$1" want="$2" label="$3"
    if [ "$got" = "$want" ]; then
        pass=$((pass + 1))
        echo "  PASS: $label"
    else
        fail=$((fail + 1))
        echo "  FAIL: $label"
        echo "    got:  $got"
        echo "    want: $want"
    fi
}

assert_contains() {
    local haystack="$1" needle="$2" label="$3"
    if [[ "$haystack" == *"$needle"* ]]; then
        pass=$((pass + 1))
        echo "  PASS: $label"
    else
        fail=$((fail + 1))
        echo "  FAIL: $label"
        echo "    missing: $needle"
        echo "    in: $haystack"
    fi
}

assert_not_contains() {
    local haystack="$1" needle="$2" label="$3"
    if [[ "$haystack" != *"$needle"* ]]; then
        pass=$((pass + 1))
        echo "  PASS: $label"
    else
        fail=$((fail + 1))
        echo "  FAIL: $label"
        echo "    unexpectedly found: $needle"
        echo "    in: $haystack"
    fi
}

# Fake just/cargo that log invocations; install can succeed or fail via env.
setup_fake_tools() {
    local bin_dir="$1"
    local install_rc="${2:-0}"
    local clean_rc="${3:-0}"
    mkdir -p "$bin_dir"
    cat >"$bin_dir/just" <<EOF
#!/usr/bin/env bash
set -euo pipefail
order_file="\${CSA_TEST_ORDER_FILE:?}"
# Log the full argv after the command name for semantic assertions.
{
    printf 'just'
    for arg in "\$@"; do
        printf ' %s' "\$arg"
    done
    printf '\\n'
} >>"\$order_file"
# Expect: just install <install_dir> (positional, not install_dir=<path>)
if [ "\${1:-}" = "install" ]; then
    exit ${install_rc}
fi
echo "unexpected just args: \$*" >&2
exit 99
EOF
    cat >"$bin_dir/cargo" <<EOF
#!/usr/bin/env bash
set -euo pipefail
order_file="\${CSA_TEST_ORDER_FILE:?}"
{
    printf 'cargo'
    for arg in "\$@"; do
        printf ' %s' "\$arg"
    done
    printf '\\n'
} >>"\$order_file"
if [ "\${1:-}" = "clean" ]; then
    exit ${clean_rc}
fi
echo "unexpected cargo args: \$*" >&2
exit 99
EOF
    chmod +x "$bin_dir/just" "$bin_dir/cargo"
}

run_hook() {
    local install_dir="$1"
    local bin_dir="$2"
    local order_file="$3"
    CSA_TEST_ORDER_FILE="$order_file" \
        CSA_POST_MERGE_INSTALL_DIR="$install_dir" \
        CSA_POST_MERGE_JUST="$bin_dir/just" \
        CSA_POST_MERGE_CARGO="$bin_dir/cargo" \
        bash "$HOOK"
}

# ---------------------------------------------------------------------------
# Real Just recipe semantics (not the fake just used for hook ordering).
# ---------------------------------------------------------------------------
echo "== real Just install argument semantics =="
tmp_semantics="$(mktemp -d)"
trap 'rm -rf "$tmp_semantics"' EXIT

pos_dry="$(cd "$ROOT" && just --dry-run install /tmp/csa-install-pos 2>&1)"
assert_contains "$pos_dry" "d='/tmp/csa-install-pos'" "positional install_dir sets d= path"
assert_not_contains "$pos_dry" 'd='\''install_dir=' "positional dry-run does not embed install_dir= prefix in d"

# Bare `install_dir=...` as a positional is wrong (historical hook bug).
bad_dry="$(cd "$ROOT" && just --dry-run install install_dir=/tmp/csa-install-bad 2>&1)"
assert_contains "$bad_dry" 'install_dir=/tmp/csa-install-bad' "bare install_dir= positional becomes the install path value"

# Shell-injection hardening: metacharacters must remain a quoted literal.
inject_dry="$(cd "$ROOT" && just --dry-run install '$(printf injected)' 2>&1)"
assert_not_contains "$inject_dry" 'd="$(printf injected)"' "install_dir is not double-quoted unescaped"
# quote() yields a single-quoted shell literal (or escaped equivalent).
assert_contains "$inject_dry" "d='" "install_dir assignment uses shell quoting"

# ---------------------------------------------------------------------------
# Hook ordering with fake tools
# ---------------------------------------------------------------------------
echo "== post-merge success: install then clean =="
tmp="$(mktemp -d)"
install_dir="$tmp/bin"
mkdir -p "$install_dir"
order="$tmp/order"
: >"$order"
setup_fake_tools "$tmp/tools" 0 0
set +e
run_hook "$install_dir" "$tmp/tools" "$order"
rc=$?
set -e
assert_eq "$rc" "0" "success exit 0"
order_body="$(cat "$order")"
assert_contains "$order_body" $'just install '"$install_dir"$'\ncargo clean' "order success install then clean"
assert_not_contains "$order_body" "install_dir=" "hook uses positional install_dir, not install_dir= keyword-as-positional"

echo "== skip when CSA_SESSION_ID set =="
tmp2="$(mktemp -d)"
install_dir2="$tmp2/bin"
mkdir -p "$install_dir2"
order2="$tmp2/order"
: >"$order2"
setup_fake_tools "$tmp2/tools" 0 0
set +e
CSA_SESSION_ID=test-session \
    CSA_TEST_ORDER_FILE="$order2" \
    CSA_POST_MERGE_INSTALL_DIR="$install_dir2" \
    CSA_POST_MERGE_JUST="$tmp2/tools/just" \
    CSA_POST_MERGE_CARGO="$tmp2/tools/cargo" \
    bash "$HOOK"
rc=$?
set -e
assert_eq "$rc" "0" "session skip exit 0"
assert_eq "$(cat "$order2")" "" "session skip no tools"

echo "== skip when install dir not writable =="
tmp3="$(mktemp -d)"
install_dir3="$tmp3/ro"
mkdir -p "$install_dir3"
chmod a-w "$install_dir3"
order3="$tmp3/order"
: >"$order3"
setup_fake_tools "$tmp3/tools" 0 0
set +e
CSA_TEST_ORDER_FILE="$order3" \
    CSA_POST_MERGE_INSTALL_DIR="$install_dir3" \
    CSA_POST_MERGE_JUST="$tmp3/tools/just" \
    CSA_POST_MERGE_CARGO="$tmp3/tools/cargo" \
    bash "$HOOK"
rc=$?
set -e
assert_eq "$rc" "0" "non-writable skip exit 0"
assert_eq "$(cat "$order3")" "" "non-writable skip no tools"
chmod u+w "$install_dir3" 2>/dev/null || true

echo "== install failure: no cargo clean =="
tmp4="$(mktemp -d)"
install_dir4="$tmp4/bin"
mkdir -p "$install_dir4"
order4="$tmp4/order"
: >"$order4"
setup_fake_tools "$tmp4/tools" 7 0
set +e
run_hook "$install_dir4" "$tmp4/tools" "$order4"
rc=$?
set -e
assert_eq "$rc" "7" "install failure propagates exit"
order_body4="$(cat "$order4")"
assert_contains "$order_body4" "just install $install_dir4" "install attempted"
assert_not_contains "$order_body4" "cargo clean" "clean skipped on install failure"

echo "== partial: install ok, clean fails =="
tmp5="$(mktemp -d)"
install_dir5="$tmp5/bin"
mkdir -p "$install_dir5"
order5="$tmp5/order"
: >"$order5"
setup_fake_tools "$tmp5/tools" 0 3
set +e
run_hook "$install_dir5" "$tmp5/tools" "$order5"
rc=$?
set -e
assert_eq "$rc" "3" "clean failure propagates exit"
order_body5="$(cat "$order5")"
assert_contains "$order_body5" $'just install '"$install_dir5"$'\ncargo clean' "order partial install then clean"

echo "== concurrent-safe order files (no clobber) =="
tmp6="$(mktemp -d)"
install_dir6="$tmp6/bin"
mkdir -p "$install_dir6"
order6a="$tmp6/order-a"
order6b="$tmp6/order-b"
: >"$order6a"
: >"$order6b"
setup_fake_tools "$tmp6/tools" 0 0
set +e
(
    CSA_TEST_ORDER_FILE="$order6a" \
        CSA_POST_MERGE_INSTALL_DIR="$install_dir6" \
        CSA_POST_MERGE_JUST="$tmp6/tools/just" \
        CSA_POST_MERGE_CARGO="$tmp6/tools/cargo" \
        bash "$HOOK"
) &
pid_a=$!
(
    CSA_TEST_ORDER_FILE="$order6b" \
        CSA_POST_MERGE_INSTALL_DIR="$install_dir6" \
        CSA_POST_MERGE_JUST="$tmp6/tools/just" \
        CSA_POST_MERGE_CARGO="$tmp6/tools/cargo" \
        bash "$HOOK"
) &
pid_b=$!
wait "$pid_a"
rc_a=$?
wait "$pid_b"
rc_b=$?
set -e
assert_eq "$rc_a" "0" "concurrent A exit 0"
assert_eq "$rc_b" "0" "concurrent B exit 0"
assert_contains "$(cat "$order6a")" "just install $install_dir6" "concurrent A logged install"
assert_contains "$(cat "$order6b")" "just install $install_dir6" "concurrent B logged install"

echo
echo "Results: $pass passed, $fail failed"
if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "All post-merge-rebuild tests passed."
