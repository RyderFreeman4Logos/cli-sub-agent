# shellcheck shell=bash
# Offline pinned-toolchain contract for the capability sandbox.
# Sourced after the isolation fixture and assertion helpers are defined.

run_offline_pinned_toolchain() {
  local fixture runner counter toolchain_root resolver_root direct_bin hook_bin
  local first second compiler_changed tool_changed missing code
  local first_identity second_identity compiler_identity tool_identity
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/offline-toolchain-counter"
  toolchain_root="$fixture/target/pinned-toolchain"
  resolver_root="$fixture/target/toolchain-resolver"
  direct_bin="$fixture/target/direct-resolver"
  hook_bin="$fixture/target/hook-resolver"
  mkdir -p "$toolchain_root/bin" "$toolchain_root/lib" "$resolver_root" \
    "$direct_bin" "$hook_bin"
  cat >"$fixture/rust-toolchain.toml" <<'EOF'
[toolchain]
channel = "9.96.0"
components = ["clippy", "rustfmt"]
EOF
  git -C "$fixture" add rust-toolchain.toml
  git -C "$fixture" commit -qm "test: pin offline fixture toolchain"

  cat >"$toolchain_root/bin/rustc" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
source="$(readlink -f "${BASH_SOURCE[0]}")"
root="$(cd "$(dirname "$source")/.." && pwd)"
case "$*" in
  '--print sysroot') printf '%s\n' "$root" ;;
  '-vV')
    cat <<'EOF'
rustc 9.96.0 (111111111 2096-05-25)
binary: rustc
commit-hash: 1111111111111111111111111111111111111111
commit-date: 2096-05-25
host: x86_64-unknown-linux-gnu
release: 9.96.0
LLVM version: 21.0.0
EOF
    ;;
  *) printf 'unexpected fixture rustc arguments\n' >&2; exit 64 ;;
esac
SH
  cat >"$toolchain_root/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  -vV) printf 'cargo 9.96.0 (222222222 2096-05-25)\n' ;;
  fmt) shift; exec cargo-fmt fmt "$@" ;;
  *) printf 'unexpected fixture cargo arguments\n' >&2; exit 64 ;;
esac
SH
  cat >"$toolchain_root/bin/cargo-fmt" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
test "${RUSTUP_TOOLCHAIN:-}" = 9.96.0-x86_64-unknown-linux-gnu
test "${CARGO_NET_OFFLINE:-}" = true
test ! -e .csa/state
awk 'NR > 1 && $1 != "lo" { exit 1 }' /proc/net/route
awk '$NF != "lo" { exit 1 }' /proc/net/ipv6_route
printf x >>target/offline-toolchain-counter
SH
  for tool in rustdoc rustfmt cargo-clippy clippy-driver; do
    printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf "%s 9.96.0\\n"\n' \
      "$tool" >"$toolchain_root/bin/$tool"
  done
  chmod +x "$toolchain_root/bin/"*

  cat >"$resolver_root/rustup" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../pinned-toolchain" && pwd)"
if [ "${1:-}" = which ]; then
  test "${2:-}" = --toolchain
  case "${3:-}" in
    9.96.0|9.96.0-x86_64-unknown-linux-gnu) ;;
    *) exit 65 ;;
  esac
  tool="${4:-}"
  test -x "$root/bin/$tool" || exit 66
  printf '%s\n' "$root/bin/$tool"
  exit 0
fi
tool="${CSA_TEST_RUSTUP_PROXY_NAME:?}"
if [ "${RUSTUP_TOOLCHAIN:-}" != 9.96.0-x86_64-unknown-linux-gnu ]; then
  echo 'info: syncing channel updates for 9.96.0-x86_64-unknown-linux-gnu' >&2
  echo 'error: offline fixture DNS denied while resolving pinned toolchain' >&2
  exit 68
fi
exec "$root/bin/$tool" "$@"
SH
  chmod +x "$resolver_root/rustup"
  printf '#!/usr/bin/env bash\n# direct launcher\nexec %q "$@"\n' \
    "$resolver_root/rustup" >"$direct_bin/rustup"
  printf '#!/usr/bin/env bash\n# hook launcher with different bytes\nexec %q "$@"\n' \
    "$resolver_root/rustup" >"$hook_bin/rustup"
  local tool launcher
  for tool in cargo cargo-fmt rustc rustdoc rustfmt cargo-clippy clippy-driver; do
    for launcher in direct hook; do
      printf '#!/usr/bin/env bash\n# %s %s proxy\nexport CSA_TEST_RUSTUP_PROXY_NAME=%q\nexec %q "$@"\n' \
        "$launcher" "$tool" "$tool" "$resolver_root/rustup" \
        >"$fixture/target/${launcher}-resolver/$tool"
    done
  done
  chmod +x "$direct_bin/"* "$hook_bin/"*

  first="$(cd "$fixture" && PATH="$direct_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  second="$(cd "$fixture" && PATH="$hook_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  first_identity="$(printf '%s' "$first" | json_field receipt_identity)"
  second_identity="$(printf '%s' "$second" | json_field receipt_identity)"
  assert_eq offline-toolchain-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq offline-toolchain-second-status reused \
    "$(printf '%s' "$second" | json_field status)"
  assert_eq offline-toolchain-entrypoint-identity "$first_identity" "$second_identity"
  assert_eq offline-toolchain-gate-runs 1 "$(wc -c <"$counter")"

  printf '# changed compiler bytes\n' >>"$toolchain_root/bin/rustc"
  compiler_changed="$(cd "$fixture" && PATH="$direct_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  compiler_identity="$(printf '%s' "$compiler_changed" | json_field receipt_identity)"
  assert_eq offline-toolchain-compiler-status executed \
    "$(printf '%s' "$compiler_changed" | json_field status)"
  assert_ne offline-toolchain-compiler-identity "$first_identity" "$compiler_identity"
  assert_eq offline-toolchain-compiler-runs 2 "$(wc -c <"$counter")"

  printf '# changed tool bytes\n' >>"$toolchain_root/bin/cargo-fmt"
  tool_changed="$(cd "$fixture" && PATH="$direct_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  tool_identity="$(printf '%s' "$tool_changed" | json_field receipt_identity)"
  assert_eq offline-toolchain-tool-status executed \
    "$(printf '%s' "$tool_changed" | json_field status)"
  assert_ne offline-toolchain-tool-identity "$compiler_identity" "$tool_identity"
  assert_eq offline-toolchain-tool-runs 3 "$(wc -c <"$counter")"

  mv "$toolchain_root/bin/cargo-fmt" "$toolchain_root/bin/cargo-fmt.missing"
  set +e
  missing="$(cd "$fixture" && PATH="$direct_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  code=$?
  set -e
  assert_eq offline-toolchain-missing-exit 125 "$code"
  assert_eq offline-toolchain-missing-status gate_failed \
    "$(printf '%s' "$missing" | json_field status)"
  assert_eq offline-toolchain-missing-reason toolchain_component_missing \
    "$(printf '%s' "$missing" | json_field rejection_reason)"
  assert_eq offline-toolchain-missing-no-gate 3 "$(wc -c <"$counter")"
  echo "PASS isolation-offline-pinned-toolchain"
}
