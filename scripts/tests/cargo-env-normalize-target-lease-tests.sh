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

# Public/non-GC hosts retain direct execution when the marker is absent.
rm "$marker"
: >"$log"
run_normalizer
[ "$(cat "$log")" = "cargo metadata target=$repo/target" ] || {
    echo "FAIL absent marker direct argv=$(cat "$log")"; exit 1;
}

# A discovered helper receives exactly `-- cargo metadata` and controls status.
cat >"$bin/cargo-target-lease" <<'EOF'
#!/usr/bin/env bash
printf 'lease' >>"${CSA_TEST_LOG:?}"
for arg in "$@"; do printf ' <%s>' "$arg" >>"${CSA_TEST_LOG:?}"; done
printf '\n' >>"${CSA_TEST_LOG:?}"
exit 23
EOF
chmod +x "$bin/cargo-target-lease"
: >"$log"
set +e
run_normalizer
lease_rc=$?
set -e
[ "$lease_rc" = 23 ] || { echo "FAIL lease status=$lease_rc"; exit 1; }
[ "$(cat "$log")" = "lease <--> <cargo> <metadata>" ] || {
    echo "FAIL lease argv=$(cat "$log")"; exit 1;
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
