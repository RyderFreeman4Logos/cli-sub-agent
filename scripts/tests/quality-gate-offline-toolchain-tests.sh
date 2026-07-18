# shellcheck shell=bash
# Offline pinned-toolchain contract for the capability sandbox.
# Sourced after the isolation fixture and assertion helpers are defined.

run_offline_pinned_toolchain() {
  local fixture runner counter toolchain_root resolver_root direct_bin hook_bin
  local nested_bin first second nested_first nested_second wrong wrong_stderr
  local compiler_changed tool_changed missing missing_stderr missing_diagnostic
  local code nested_code nested_reason wrong_code wrong_diagnostic
  local first_identity second_identity compiler_identity tool_identity
  local self_location_bin self_location_calls wrong_location_parent
  local toolchain_root_literal wrong_location_root wrong_location_root_literal
  local wrong_location_parent_literal self_location_calls_literal first_code
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/offline-toolchain-counter"
  toolchain_root="$fixture/target/pinned-toolchain"
  resolver_root="$fixture/target/toolchain-resolver"
  direct_bin="$fixture/target/direct-resolver"
  hook_bin="$fixture/target/hook-resolver"
  nested_bin="$fixture/target/nested-outer-closure"
  self_location_bin="$fixture/target/wrong-self-location-bin"
  self_location_calls="$fixture/target/wrong-self-location.calls"
  wrong_location_parent="$fixture/target/wrong-self-location"
  wrong_location_root="$wrong_location_parent/pinned-toolchain"
  mkdir -p "$toolchain_root/bin" "$toolchain_root/lib" "$resolver_root" \
    "$direct_bin" "$hook_bin" "$nested_bin" "$self_location_bin" \
    "$wrong_location_parent/locator" "$wrong_location_root/bin"
  cat >"$fixture/rust-toolchain.toml" <<'EOF'
[toolchain]
channel = "9.96.0"
components = ["clippy", "rustfmt"]
EOF
  git -C "$fixture" add rust-toolchain.toml
  git -C "$fixture" commit -qm "test: pin offline fixture toolchain"

  # A sealed launcher has no trustworthy source path. Poison legacy helper-based
  # self-location so fixture roots must come from declared literals.
  printf -v toolchain_root_literal '%q' "$toolchain_root"
  printf -v self_location_calls_literal '%q' "$self_location_calls"
  printf -v wrong_location_root_literal '%q' "$wrong_location_root"
  printf -v wrong_location_parent_literal '%q' "$wrong_location_parent"
  cat >"$self_location_bin/readlink" <<EOF
#!/usr/bin/env bash
set -euo pipefail
calls=$self_location_calls_literal
wrong_root=$wrong_location_root_literal
test "\$#" -eq 2
test "\${1:-}" = -f
printf 'readlink\n' >>"\$calls"
printf '%s\n' "\$wrong_root/bin/rustc"
EOF
  cat >"$self_location_bin/dirname" <<EOF
#!/usr/bin/env bash
set -euo pipefail
calls=$self_location_calls_literal
wrong_root=$wrong_location_root_literal
wrong_parent=$wrong_location_parent_literal
test "\$#" -eq 1
printf 'dirname\n' >>"\$calls"
case "\${1##*/}" in
  rustc) printf '%s\n' "\$wrong_root/bin" ;;
  *) printf '%s\n' "\$wrong_parent/locator" ;;
esac
EOF
  chmod +x "$self_location_bin/readlink" "$self_location_bin/dirname"

  cat >"$toolchain_root/bin/rustc" <<EOF
#!/usr/bin/env bash
set -euo pipefail
root=$toolchain_root_literal
case "\$*" in
  '--print sysroot')
    case "\$0" in
      /run/csa-bin/rustc|/run/csa-rust-toolchain/bin/rustc)
        printf '%s\n' /run/csa-rust-toolchain
        ;;
      *) printf '%s\n' "\$root" ;;
    esac
    ;;
  '-vV')
    cat <<'VERSION'
rustc 9.96.0 (111111111 2096-05-25)
binary: rustc
commit-hash: 1111111111111111111111111111111111111111
commit-date: 2096-05-25
host: x86_64-unknown-linux-gnu
release: 9.96.0
LLVM version: 21.0.0
VERSION
    ;;
  *) printf 'unexpected fixture rustc arguments\n' >&2; exit 64 ;;
esac
EOF
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

  cat >"$resolver_root/rustup" <<EOF
#!/usr/bin/env bash
set -euo pipefail
root=$toolchain_root_literal
if [ "\${1:-}" = which ]; then
  test "\${2:-}" = --toolchain
  case "\${3:-}" in
    9.96.0|9.96.0-x86_64-unknown-linux-gnu) ;;
    *) exit 65 ;;
  esac
  tool="\${4:-}"
  test -x "\$root/bin/\$tool" || exit 66
  printf '%s\n' "\$root/bin/\$tool"
  exit 0
fi
tool="\${CSA_TEST_RUSTUP_PROXY_NAME:?}"
if [ "\${RUSTUP_TOOLCHAIN:-}" != 9.96.0-x86_64-unknown-linux-gnu ]; then
  echo 'info: syncing channel updates for 9.96.0-x86_64-unknown-linux-gnu' >&2
  echo 'error: offline fixture DNS denied while resolving pinned toolchain' >&2
  exit 68
fi
exec "\$root/bin/\$tool" "\$@"
EOF
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
  for tool in cargo cargo-fmt rustc rustdoc rustfmt cargo-clippy clippy-driver; do
    ln -s "$toolchain_root/bin/$tool" "$nested_bin/$tool"
  done
  cat >"$nested_bin/rustup" <<'SH'
#!/usr/bin/env bash
echo 'unexpected ambient rustup resolver use' >&2
exit 66
SH
  chmod +x "$nested_bin/rustup"

  set +e
  first="$(cd "$fixture" && PATH="$direct_bin:$self_location_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  first_code=$?
  set -e
  if [ "$first_code" -ne 0 ]; then
    assert_path_exists offline-toolchain-self-location-sentinel-triggered \
      "$self_location_calls"
  fi
  assert_eq offline-toolchain-first-exit 0 "$first_code"
  second="$(cd "$fixture" && PATH="$hook_bin:$self_location_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  first_identity="$(printf '%s' "$first" | json_field receipt_identity)"
  second_identity="$(printf '%s' "$second" | json_field receipt_identity)"
  assert_eq offline-toolchain-first-status executed \
    "$(printf '%s' "$first" | json_field status)"
  assert_eq offline-toolchain-second-status executed \
    "$(printf '%s' "$second" | json_field status)"
  assert_ne offline-toolchain-entrypoint-identity "$first_identity" "$second_identity"
  assert_eq offline-toolchain-gate-runs 2 "$(wc -c <"$counter")"

  set +e
  nested_first="$(cd "$fixture" && \
    PATH="$nested_bin:$self_location_bin:/usr/bin:/bin" \
    RUSTUP_TOOLCHAIN=9.96.0-x86_64-unknown-linux-gnu \
    "$runner" -- cargo fmt --nested-outer-receipt-case)"
  nested_code=$?
  set -e
  if [ "$nested_code" -ne 0 ]; then
    nested_reason="$(printf '%s' "$nested_first" | json_field rejection_reason)"
    assert_eq nested-static-outer-receipt-exact-reuse-reason \
      toolchain_component_missing "$nested_reason"
  fi
  assert_eq nested-static-outer-receipt-exact-reuse-exit 0 "$nested_code"
  nested_second="$(cd "$fixture" && \
    PATH="$nested_bin:$self_location_bin:/usr/bin:/bin" \
    RUSTUP_TOOLCHAIN=9.96.0-x86_64-unknown-linux-gnu \
    "$runner" -- cargo fmt --nested-outer-receipt-case)"
  assert_eq nested-static-outer-receipt-first-status executed \
    "$(printf '%s' "$nested_first" | json_field status)"
  assert_eq nested-static-outer-receipt-second-status reused \
    "$(printf '%s' "$nested_second" | json_field status)"
  assert_eq nested-static-outer-receipt-identity \
    "$(printf '%s' "$nested_first" | json_field receipt_identity)" \
    "$(printf '%s' "$nested_second" | json_field receipt_identity)"
  assert_eq nested-static-outer-receipt-gate-runs 3 "$(wc -c <"$counter")"

  wrong_stderr="$fixture/target/wrong-toolchain.stderr"
  set +e
  wrong="$(cd "$fixture" && \
    PATH="$nested_bin:$self_location_bin:/usr/bin:/bin" \
    RUSTUP_TOOLCHAIN=9.95.0-x86_64-unknown-linux-gnu \
    "$runner" -- cargo fmt --wrong-nested-selector 2>"$wrong_stderr")"
  wrong_code=$?
  set -e
  wrong_diagnostic="$(<"$wrong_stderr")"
  assert_eq nested-static-outer-wrong-toolchain-exit 125 "$wrong_code"
  assert_eq nested-static-outer-wrong-toolchain-status gate_failed \
    "$(printf '%s' "$wrong" | json_field status)"
  assert_eq nested-static-outer-wrong-toolchain-reason toolchain_invalid \
    "$(printf '%s' "$wrong" | json_field rejection_reason)"
  assert_eq nested-static-outer-wrong-toolchain-diagnostic \
    'ERROR quality-gate status=gate_failed exit=125 reason=toolchain_invalid' \
    "$wrong_diagnostic"
  assert_eq nested-static-outer-wrong-toolchain-no-gate 3 \
    "$(wc -c <"$counter")"

  printf '# changed compiler bytes\n' >>"$toolchain_root/bin/rustc"
  compiler_changed="$(cd "$fixture" && PATH="$direct_bin:$self_location_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  compiler_identity="$(printf '%s' "$compiler_changed" | json_field receipt_identity)"
  assert_eq offline-toolchain-compiler-status executed \
    "$(printf '%s' "$compiler_changed" | json_field status)"
  assert_ne offline-toolchain-compiler-identity "$first_identity" "$compiler_identity"
  assert_eq offline-toolchain-compiler-runs 4 "$(wc -c <"$counter")"

  printf '# changed tool bytes\n' >>"$toolchain_root/bin/cargo-fmt"
  tool_changed="$(cd "$fixture" && PATH="$direct_bin:$self_location_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check)"
  tool_identity="$(printf '%s' "$tool_changed" | json_field receipt_identity)"
  assert_eq offline-toolchain-tool-status executed \
    "$(printf '%s' "$tool_changed" | json_field status)"
  assert_ne offline-toolchain-tool-identity "$compiler_identity" "$tool_identity"
  assert_eq offline-toolchain-tool-runs 5 "$(wc -c <"$counter")"

  mv "$toolchain_root/bin/cargo-fmt" "$toolchain_root/bin/cargo-fmt.missing"
  missing_stderr="$fixture/target/missing-toolchain.stderr"
  set +e
  missing="$(cd "$fixture" && PATH="$direct_bin:$self_location_bin:$PATH" \
    "$runner" -- cargo fmt --all -- --check 2>"$missing_stderr")"
  code=$?
  set -e
  missing_diagnostic="$(<"$missing_stderr")"
  assert_eq offline-toolchain-missing-exit 125 "$code"
  assert_eq offline-toolchain-missing-status gate_failed \
    "$(printf '%s' "$missing" | json_field status)"
  assert_eq offline-toolchain-missing-reason toolchain_component_missing \
    "$(printf '%s' "$missing" | json_field rejection_reason)"
  assert_eq offline-toolchain-missing-diagnostic \
    'ERROR quality-gate status=gate_failed exit=125 reason=toolchain_component_missing' \
    "$missing_diagnostic"
  assert_eq offline-toolchain-missing-diagnostic-lines 1 \
    "$(wc -l <"$missing_stderr")"
  assert_not_matches offline-toolchain-missing-diagnostic-sanitized \
    '(/home/|/tmp/|PASSWORD|SECRET|TOKEN)' "$missing_diagnostic"
  assert_eq offline-toolchain-missing-no-gate 5 "$(wc -c <"$counter")"
  assert_path_absent offline-toolchain-no-self-location-coupling \
    "$self_location_calls"
  echo "PASS isolation-offline-pinned-toolchain"
}

toolchain_projection() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)["provenance"]["toolchain"])'
}

run_multicall_toolchain() {
  local fixture runner counter toolchain_root multicall parent_bin selector_bin direct_bin
  local nested_bin selector first second identity first_projection second_projection
  local direct_terminal_output direct_terminal_code nested_first
  local nested_second nested_identity nested_poison nested_poison_identity
  fixture="$(new_isolation_fixture)"
  runner="$fixture/scripts/hooks/quality-gate-receipt.sh"
  counter="$fixture/target/multicall-counter"
  toolchain_root="$fixture/target/multicall-toolchain"
  multicall="$fixture/target/multicall-terminal"
  parent_bin="$fixture/target/multicall-parent-bin"
  selector_bin="$fixture/target/multicall-selector-bin"
  direct_bin="$fixture/target/multicall-direct-bin"
  nested_bin="$fixture/target/multicall-nested-bin"
  selector="9.97.0-x86_64-unknown-linux-gnu"
  mkdir -p "$toolchain_root/bin" "$toolchain_root/lib" "$parent_bin" \
    "$selector_bin" "$direct_bin" "$nested_bin"
  cat >"$fixture/rust-toolchain.toml" <<'EOF'
[toolchain]
channel = "9.97.0"
components = ["clippy", "rustfmt"]
EOF
  git -C "$fixture" add rust-toolchain.toml
  git -C "$fixture" commit -qm "test: pin multicall fixture toolchain"

  cat >"$toolchain_root/bin/rustc" <<EOF
#!/usr/bin/env bash
set -euo pipefail
case "\${1:-}" in
  -vV)
    cat <<'VERSION'
rustc 9.97.0 (222222222 2097-05-25)
binary: rustc
commit-hash: 2222222222222222222222222222222222222222
commit-date: 2097-05-25
host: x86_64-unknown-linux-gnu
release: 9.97.0
LLVM version: 22.2.0
VERSION
    ;;
  --print)
    test "\${2:-}" = sysroot
    case "\$(readlink -f "\$0")" in
      /run/csa-rust-toolchain/bin/rustc) printf '%s\\n' /run/csa-rust-toolchain ;;
      *) printf '%s\\n' "$toolchain_root" ;;
    esac
    ;;
  *) exit 64 ;;
esac
EOF
  cat >"$toolchain_root/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  -vV) printf 'cargo 9.97.0\n' ;;
  fmt) shift; exec cargo-fmt fmt "$@" ;;
  *) exit 64 ;;
esac
SH
  cat >"$toolchain_root/bin/cargo-fmt" <<EOF
#!/usr/bin/env bash
set -euo pipefail
test "\${1:-}" = fmt
test "\${RUSTUP_TOOLCHAIN:-}" = "$selector"
test "\${CARGO_NET_OFFLINE:-}" = true
test "\$(command -v cargo)" = /run/csa-bin/cargo
test "\$(rustc --print sysroot)" = /run/csa-rust-toolchain
printf x >>"$counter"
EOF
  for tool in cargo-clippy clippy-driver rustdoc rustfmt; do
    cat >"$toolchain_root/bin/$tool" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf '%s 9.97.0\\n' "$tool"
EOF
  done
  chmod +x "$toolchain_root/bin/"*

  cat >"$multicall" <<EOF
#!/usr/bin/env bash
set -euo pipefail
root="$toolchain_root"
case "\$(basename "\$0")" in
  rustup)
    test "\${1:-}" = which
    test "\${2:-}" = --toolchain
    case "\${3:-}" in
      9.97.0|$selector) ;;
      *) exit 65 ;;
    esac
    tool="\${4:-}"
    test -x "\$root/bin/\$tool"
    printf '%s\\n' "\$root/bin/\$tool"
    ;;
  rustc)
    test "\${RUSTUP_TOOLCHAIN:-}" = "$selector"
    exec "\$root/bin/rustc" "\$@"
    ;;
  cargo|cargo-clippy|cargo-fmt|clippy-driver|rustdoc|rustfmt)
    exec "\$root/bin/\$(basename "\$0")" "\$@"
    ;;
  *)
    printf 'unexpected multicall invocation: %s\\n' "\$0" >&2
    exit 64
    ;;
esac
EOF
  chmod +x "$multicall"
  local tool
  for tool in rustup cargo cargo-clippy cargo-fmt clippy-driver rustc rustdoc rustfmt; do
    ln -s "$multicall" "$parent_bin/$tool"
    ln -s "$multicall" "$selector_bin/$tool"
  done
  for tool in cargo cargo-clippy cargo-fmt clippy-driver rustc rustdoc rustfmt; do
    ln -s "$toolchain_root/bin/$tool" "$direct_bin/$tool"
    ln -s "$toolchain_root/bin/$tool" "$nested_bin/$tool"
  done
  cat >"$nested_bin/rustup" <<'SH'
#!/usr/bin/env bash
printf 'unexpected nested rustup use\n' >&2
exit 66
SH
  chmod +x "$nested_bin/rustup"

  direct_terminal_output="$(PATH="$parent_bin:/usr/bin:/bin" \
    "$parent_bin/rustup" which --toolchain 9.97.0 rustc)"
  assert_eq multicall-alias-control-path "$toolchain_root/bin/rustc" \
    "$direct_terminal_output"
  set +e
  PATH="$parent_bin:/usr/bin:/bin" "$multicall" which --toolchain 9.97.0 rustc \
    >/dev/null 2>&1
  direct_terminal_code=$?
  set -e
  assert_ne multicall-alias-control-terminal-fails 0 "$direct_terminal_code"
  echo "PASS isolation-multicall-a0-alias-control"

  first="$(cd "$fixture" && env -u RUSTUP_TOOLCHAIN \
    PATH="$parent_bin:/usr/bin:/bin" "$runner" -- cargo fmt --all -- --check)"
  second="$(cd "$fixture" && env -u RUSTUP_TOOLCHAIN \
    PATH="$parent_bin:/usr/bin:/bin" "$runner" -- cargo fmt --all -- --check)"
  identity="$(printf '%s' "$first" | json_field receipt_identity)"
  first_projection="$(printf '%s' "$first" | toolchain_projection)"
  assert_eq multicall-p0-first-status executed "$(printf '%s' "$first" | json_field status)"
  assert_eq multicall-p0-second-status reused "$(printf '%s' "$second" | json_field status)"
  assert_eq multicall-p0-identity "$identity" \
    "$(printf '%s' "$second" | json_field receipt_identity)"
  assert_eq multicall-p0-gate-runs 1 "$(wc -c <"$counter")"
  assert_eq multicall-p0-projection \
    'selector-unset;invocation-rustup;terminal-multicall-terminal;terminal-digest-sha256;sysroot-verified-host-to-static;mount-depth-1;umask-022;query-launchers-2' \
    "$first_projection"
  echo "PASS isolation-multicall-p0-literal-parent first=executed projection=$first_projection"
  echo "PASS isolation-multicall-o0-outer-remap first=executed projection=$first_projection"

  first="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$selector_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --all -- --check)"
  second="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$selector_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --all -- --check)"
  second_projection="$(printf '%s' "$first" | toolchain_projection)"
  assert_eq multicall-p1-first-status executed "$(printf '%s' "$first" | json_field status)"
  assert_eq multicall-p1-second-status reused "$(printf '%s' "$second" | json_field status)"
  assert_eq multicall-p1-gate-runs 2 "$(wc -c <"$counter")"
  assert_eq multicall-p1-projection \
    'selector-exact;invocation-rustc;terminal-multicall-terminal;terminal-digest-sha256;sysroot-verified-host-to-static;mount-depth-1;umask-022;query-launchers-1' \
    "$second_projection"
  echo "PASS isolation-multicall-p1-selector-shim first=executed projection=$second_projection"

  first="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$direct_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --all -- --check)"
  second="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$direct_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --all -- --check)"
  assert_eq multicall-d0-first-status executed "$(printf '%s' "$first" | json_field status)"
  assert_eq multicall-d0-second-status reused "$(printf '%s' "$second" | json_field status)"
  assert_eq multicall-d0-gate-runs 3 "$(wc -c <"$counter")"
  echo "PASS isolation-multicall-d0-direct-closure first=executed projection=$(printf '%s' "$first" | toolchain_projection)"

  nested_first="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$nested_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --nested-multicall-remap)"
  nested_second="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$nested_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --nested-multicall-remap)"
  nested_identity="$(printf '%s' "$nested_first" | json_field receipt_identity)"
  assert_eq multicall-n0-first-status executed \
    "$(printf '%s' "$nested_first" | json_field status)"
  assert_eq multicall-n0-second-status reused \
    "$(printf '%s' "$nested_second" | json_field status)"
  assert_eq multicall-n0-gate-runs 4 "$(wc -c <"$counter")"
  echo "PASS isolation-multicall-n0-nested-remap first=executed projection=$(printf '%s' "$nested_first" | toolchain_projection)"

  printf '\n# authority poison parent\n' >>"$multicall"
  first="$(cd "$fixture" && env -u RUSTUP_TOOLCHAIN \
    PATH="$parent_bin:/usr/bin:/bin" "$runner" -- cargo fmt --all -- --check)"
  assert_eq multicall-r0-parent-status executed "$(printf '%s' "$first" | json_field status)"
  assert_ne multicall-r0-parent-identity "$identity" \
    "$(printf '%s' "$first" | json_field receipt_identity)"
  assert_eq multicall-r0-parent-gate-runs 5 "$(wc -c <"$counter")"

  printf '\n# authority poison nested\n' >>"$toolchain_root/bin/rustc"
  nested_poison="$(cd "$fixture" && \
    RUSTUP_TOOLCHAIN="$selector" PATH="$nested_bin:/usr/bin:/bin" \
    "$runner" -- cargo fmt --nested-multicall-remap)"
  nested_poison_identity="$(printf '%s' "$nested_poison" | json_field receipt_identity)"
  assert_eq multicall-r0-nested-status executed \
    "$(printf '%s' "$nested_poison" | json_field status)"
  assert_ne multicall-r0-nested-identity "$nested_identity" "$nested_poison_identity"
  assert_eq multicall-r0-nested-gate-runs 6 "$(wc -c <"$counter")"
  echo "PASS isolation-multicall-r0-authority-poison first=executed projection=$(printf '%s' "$nested_poison" | toolchain_projection)"
}
