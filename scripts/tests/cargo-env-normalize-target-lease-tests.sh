#!/usr/bin/env bash
# Target-GC lease launcher contract for cargo-env-normalize.sh (#2964).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
bin="$tmp/bin"
mirror="$tmp/mirror"
normalizer="$tmp/cargo-env-normalize.sh"
log="$tmp/log"
home="$tmp/home"
mkdir -p "$repo" "$bin" "$home"
cp "$ROOT/scripts/cargo-env-normalize.sh" "$normalizer"
python3 - "$normalizer" "$mirror" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
path.write_text(path.read_text().replace('/ssd/mirror-rootfs', sys.argv[2]))
PY

cat >"$bin/git" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "${CSA_TEST_REPO_ROOT:?}"
EOF
cat >"$bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s target=%s\n' "$*" "${CARGO_TARGET_DIR:?}" >>"${CSA_TEST_LOG:?}"
EOF
chmod +x "$bin/git" "$bin/cargo"

run_normalizer() {
    env \
        PATH="$bin:/usr/bin:/bin" \
        HOME="$home" \
        CSA_TEST_REPO_ROOT="$repo" \
        CSA_TEST_LOG="$log" \
        CARGO_HOME="$tmp/cargo-home" \
        CARGO_INSTALL_ROOT="$tmp/cargo-install-root" \
        RUSTUP_HOME="$tmp/rustup-home" \
        bash "$normalizer" cargo metadata
}

run_normalizer_with_default_install_root() {
    env -u CARGO_INSTALL_ROOT \
        PATH="$bin:/usr/bin:/bin" \
        HOME="$home" \
        CSA_TEST_REPO_ROOT="$repo" \
        CSA_TEST_LOG="$log" \
        CARGO_HOME="$tmp/cargo-home" \
        RUSTUP_HOME="$tmp/rustup-home" \
        bash "$normalizer" cargo metadata
}

marker="$mirror$repo/.rust-target-gc-admission-v1"

# Missing helper + managed marker must stop before Cargo.
mkdir -p "$(dirname "$marker")"
touch "$marker"
: >"$log"
set +e
missing_output="$(run_normalizer 2>&1)"
missing_rc=$?
set -e
[ "$missing_rc" = 1 ] || { echo "FAIL missing helper status=$missing_rc"; exit 1; }
[[ "$missing_output" == *"target GC admission helper is unavailable"* ]] || {
    echo "FAIL missing helper diagnostic=$missing_output"; exit 1;
}
[ ! -s "$log" ] || { echo "FAIL Cargo started without target lease helper"; exit 1; }

cat >"$bin/cargo-target-lease" <<'EOF'
#!/usr/bin/env bash
if [ -e "${CSA_TEST_REPO_ROOT:?}/target" ]; then
    printf 'pre-lease-target-mutation\n' >>"${CSA_TEST_LOG:?}"
    exit 97
fi
printf 'lease' >>"${CSA_TEST_LOG:?}"
for arg in "$@"; do printf ' <%s>' "$arg" >>"${CSA_TEST_LOG:?}"; done
printf ' target=<%s>\n' "${CARGO_TARGET_DIR:?}" >>"${CSA_TEST_LOG:?}"
exit 23
EOF
chmod +x "$bin/cargo-target-lease"

# A PATH helper is ignored on an unprovisioned public host; Cargo gets the
# original argv and logical workspace target directly.
rm -rf "$mirror$repo"
: >"$log"
run_normalizer
[ "$(cat "$log")" = "cargo metadata target=$repo/target" ] || {
    echo "FAIL public host direct argv=$(cat "$log")"; exit 1;
}

# An explicit helper remains forced even when the canonical parent is absent.
: >"$log"
set +e
CSA_CARGO_TARGET_LEASE="$bin/cargo-target-lease" run_normalizer
explicit_rc=$?
set -e
[ "$explicit_rc" = 23 ] || { echo "FAIL explicit lease status=$explicit_rc"; exit 1; }
[ "$(cat "$log")" = "lease <--> <cargo> <metadata> target=<$mirror$repo/target>" ] || {
    echo "FAIL explicit lease argv=$(cat "$log")"; exit 1;
}

# The default install root is created only by the command running under the
# lease, never while the helper still waits to acquire it.
rm -rf "$repo/target"
: >"$log"
set +e
CSA_CARGO_TARGET_LEASE="$bin/cargo-target-lease" run_normalizer_with_default_install_root
install_root_rc=$?
set -e
[ "$install_root_rc" = 23 ] || {
    echo "FAIL pre-lease install-root mutation status=$install_root_rc log=$(cat "$log")"; exit 1;
}
[ ! -e "$repo/target" ] || { echo "FAIL target mutated before lease helper"; exit 1; }
[ "$(cat "$log")" = "lease <--> </bin/sh> <-c> <mkdir -p \"\$CARGO_INSTALL_ROOT\"; exec \"\$@\"> <cargo-env-normalize> <cargo> <metadata> target=<$mirror$repo/target>" ] || {
    echo "FAIL default install-root lease argv=$(cat \"$log\")"; exit 1;
}

# PATH discovery is enabled once the lexical canonical parent exists.
mkdir -p "$mirror$repo"
: >"$log"
set +e
run_normalizer
lease_rc=$?
set -e
[ "$lease_rc" = 23 ] || { echo "FAIL lease status=$lease_rc"; exit 1; }
[ "$(cat "$log")" = "lease <--> <cargo> <metadata> target=<$mirror$repo/target>" ] || {
    echo "FAIL lease argv=$(cat "$log")"; exit 1;
}

# A checked-in normalizer invoked from a repository subdirectory must give a
# s-compatible helper the root parent, never a subdirectory-derived parent.
subdir="$repo/crates/member"
mkdir -p "$subdir"
cat >"$bin/cargo-target-lease" <<'EOF'
#!/usr/bin/env bash
parent="${CARGO_TARGET_DIR%/target}"
touch "$parent/.rust-target-gc-admission-v1"
printf 'lease' >>"${CSA_TEST_LOG:?}"
for arg in "$@"; do printf ' <%s>' "$arg" >>"${CSA_TEST_LOG:?}"; done
printf ' target=<%s>\n' "${CARGO_TARGET_DIR:?}" >>"${CSA_TEST_LOG:?}"
exit 23
EOF
chmod +x "$bin/cargo-target-lease"
: >"$log"
set +e
(cd "$subdir" && run_normalizer)
subdir_rc=$?
set -e
[ "$subdir_rc" = 23 ] || { echo "FAIL subdir lease status=$subdir_rc"; exit 1; }
[ "$(cat "$log")" = "lease <--> <cargo> <metadata> target=<$mirror$repo/target>" ] || {
    echo "FAIL subdir lease argv=$(cat "$log")"; exit 1;
}
[ -e "$marker" ] || { echo "FAIL root marker missing"; exit 1; }
[ ! -e "$mirror$subdir/.rust-target-gc-admission-v1" ] || {
    echo "FAIL subdir marker created"; exit 1;
}

# Scratch target preservation deliberately bypasses the helper, even with marker.
touch "$marker"
scratch="$tmp/scratch"
mkdir -p "$scratch"
: >"$log"
env \
    PATH="$bin:/usr/bin:/bin" \
    HOME="$home" \
    CSA_TEST_REPO_ROOT="$repo" \
    CSA_TEST_LOG="$log" \
    CSA_PRESERVE_CARGO_TARGET_DIR=1 \
    CARGO_TARGET_DIR="$scratch" \
    CARGO_HOME="$tmp/cargo-home" \
    CARGO_INSTALL_ROOT="$tmp/cargo-install-root" \
    RUSTUP_HOME="$tmp/rustup-home" \
    bash "$normalizer" cargo metadata
[ "$(cat "$log")" = "cargo metadata target=$scratch" ] || {
    echo "FAIL preserve bypass=$(cat "$log")"; exit 1;
}

echo "cargo-env-normalize target lease tests passed."
