#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
builder="${repo_root}/scripts/build-exact-head-binaries.sh"
tmp="$(mktemp -d)"
race_builder_pid=""
cleanup() {
  if [ -n "${race_builder_pid}" ]; then
    kill "${race_builder_pid}" 2>/dev/null || true
    wait "${race_builder_pid}" 2>/dev/null || true
  fi
  rm -rf "${tmp}"
}
trap cleanup EXIT

fixture="${tmp}/fixture"
output=""
mkdir -p \
  "${fixture}/scripts" \
  "${fixture}/crates/cli-sub-agent/src" \
  "${fixture}/crates/weave/src"
git -C "${tmp}" init -q fixture
git -C "${fixture}" config user.name "Exact Build Test"
git -C "${fixture}" config user.email "exact-build@example.invalid"

cat >"${fixture}/.gitignore" <<'EOF'
.cargo/
.env
EOF
cat >"${fixture}/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/cli-sub-agent", "crates/weave"]
resolver = "2"
EOF
cat >"${fixture}/crates/cli-sub-agent/Cargo.toml" <<'EOF'
[package]
name = "cli-sub-agent"
version = "0.0.0"
edition = "2024"
build = "build.rs"

[[bin]]
name = "csa"
path = "src/main.rs"
EOF
cat >"${fixture}/crates/cli-sub-agent/build.rs" <<'EOF'
fn main() {
    std::thread::sleep(std::time::Duration::from_secs(2));
}
EOF
cat >"${fixture}/crates/cli-sub-agent/src/main.rs" <<'EOF'
fn main() {
    println!("exact archive binary: csa");
}
EOF
cat >"${fixture}/crates/weave/Cargo.toml" <<'EOF'
[package]
name = "weave"
version = "0.0.0"
edition = "2024"
EOF
cat >"${fixture}/crates/weave/src/main.rs" <<'EOF'
fn main() {
    println!("exact archive binary: weave");
}
EOF
cp "${repo_root}/scripts/cargo-env-normalize.sh" \
  "${fixture}/scripts/cargo-env-normalize.sh"
cp "${repo_root}/scripts/resolve-trusted-cargo.sh" \
  "${fixture}/scripts/resolve-trusted-cargo.sh"
chmod +x \
  "${fixture}/scripts/cargo-env-normalize.sh" \
  "${fixture}/scripts/resolve-trusted-cargo.sh"
cargo_bin="$("${fixture}/scripts/resolve-trusted-cargo.sh" --repo "${fixture}")"
rustup_home="${tmp}/rustup-only-home"
mkdir -p "${rustup_home}/.cargo/bin"
cat >"${rustup_home}/.cargo/bin/cargo" <<'EOF'
#!/bin/sh
[ "${1:-}" = "--version" ] || exit 2
printf '%s\n' 'cargo 1.0.0 (rustup-only fixture)'
EOF
chmod +x "${rustup_home}/.cargo/bin/cargo"
resolved_rustup_cargo="$(
  HOME="${rustup_home}" "${fixture}/scripts/resolve-trusted-cargo.sh" \
    --repo "${fixture}" --home-only
)"
[ "${resolved_rustup_cargo}" = "${rustup_home}/.cargo/bin/cargo" ]
env -u RUSTC_WRAPPER \
  -u RUSTC_WORKSPACE_WRAPPER \
  -u RUSTFLAGS \
  -u CARGO_ENCODED_RUSTFLAGS \
  -u RUSTC_BOOTSTRAP \
  "${cargo_bin}" generate-lockfile --manifest-path "${fixture}/Cargo.toml"
git -C "${fixture}" add .
git -C "${fixture}" commit -qm "fixture"
head="$(git -C "${fixture}" rev-parse HEAD)"
output="${fixture}/target/exact-head/${head}"

victim="${tmp}/victim"
mkdir -p "${victim}" "${fixture}/target/exact-head"
printf 'keep\n' >"${victim}/sentinel"
ln -s "${victim}" "${fixture}/target/exact-head/escape"
for dangerous_output in \
  "${HOME}" \
  "${fixture}" \
  "${fixture}/target/exact-head" \
  "${fixture}/target/exact-head/not-the-reviewed-head" \
  "${fixture}/target/exact-head/../../.." \
  "${fixture}/target/exact-head/escape"; do
  if "${builder}" \
    --repo "${fixture}" \
    --head "${head}" \
    --output-dir "${dangerous_output}" \
    >"${tmp}/dangerous-output.stdout" 2>"${tmp}/dangerous-output.stderr"; then
    echo "ERROR: accepted dangerous exact-build output path: ${dangerous_output}" >&2
    exit 1
  fi
  grep -q 'must resolve to the exact commit output path' "${tmp}/dangerous-output.stderr"
  [ "$(cat "${victim}/sentinel")" = "keep" ]
done

mkdir -p "${output}"
printf 'unrelated\n' >"${output}/sentinel"
if "${builder}" \
  --repo "${fixture}" \
  --head "${head}" \
  --output-dir "${output}" \
  >"${tmp}/unmarked-output.stdout" 2>"${tmp}/unmarked-output.stderr"; then
  echo "ERROR: replaced unmarked exact-build output directory" >&2
  exit 1
fi
grep -q 'refusing to replace unmarked or invalid exact-build output' \
  "${tmp}/unmarked-output.stderr"
[ "$(cat "${output}/sentinel")" = "unrelated" ]
rm -rf "${output}"

race_tmp="${tmp}/race-tmp"
mkdir -p "${race_tmp}"
TMPDIR="${race_tmp}" "${builder}" \
  --repo "${fixture}" \
  --head "${head}" \
  --output-dir "${output}" \
  >"${tmp}/race-output.stdout" 2>"${tmp}/race-output.stderr" &
race_builder_pid="$!"
scratch_seen=false
for _ in $(seq 1 500); do
  for candidate in "${race_tmp}"/csa-exact-build.*; do
    if [ -d "${candidate}" ]; then
      scratch_seen=true
      break 2
    fi
  done
  sleep 0.01
done
if [ "${scratch_seen}" != true ]; then
  echo "ERROR: exact-build race fixture never reached the build barrier" >&2
  exit 1
fi
mkdir -p "${output}"
printf 'raced-in\n' >"${output}/sentinel"
if wait "${race_builder_pid}"; then
  race_builder_pid=""
  echo "ERROR: exact builder replaced output created after initial validation" >&2
  exit 1
fi
race_builder_pid=""
grep -q 'refusing to replace unmarked or invalid exact-build output' \
  "${tmp}/race-output.stderr"
[ "$(cat "${output}/sentinel")" = "raced-in" ]
rm -rf "${output}"
mkdir -p "${fixture}/.cargo"
cat >"${fixture}/.cargo/config.toml" <<'EOF'
[build]
rustc-wrapper = "/definitely/not/the-reviewed-wrapper"
rustflags = ["--cfg", "contaminated_checkout"]
EOF
cat >"${fixture}/.env" <<'EOF'
RUSTC_WRAPPER=/definitely/not/the-reviewed-wrapper
RUSTFLAGS=--cfg contaminated_dotenv
EOF
hostile_tmp="${tmp}/hostile-tmp"
mkdir -p "${hostile_tmp}/.cargo"
cat >"${hostile_tmp}/.cargo/config.toml" <<'EOF'
[build]
rustc-wrapper = "/definitely/not/the-ancestor-wrapper"
EOF

TMPDIR="${hostile_tmp}" \
RUSTC_WRAPPER=/live/wrapper \
RUSTC_WORKSPACE_WRAPPER=/live/workspace-wrapper \
RUSTFLAGS='--cfg contaminated_env' \
CARGO_ENCODED_RUSTFLAGS='--cfgcontaminated_encoded' \
RUSTC_BOOTSTRAP=1 \
CARGO_PROFILE_RELEASE_OPT_LEVEL=0 \
CARGO_HOME="${fixture}/.cargo" \
  "${builder}" --repo "${fixture}" --head "${head}" --output-dir "${output}"

[ "$(cat "${output}/SOURCE_COMMIT")" = "${head}" ]
[ "$("${output}/csa")" = "exact archive binary: csa" ]
[ "$("${output}/weave")" = "exact archive binary: weave" ]
"${builder}" --repo "${fixture}" --head "${head}" --output-dir "${output}"
[ "$(cat "${output}/SOURCE_COMMIT")" = "${head}" ]
printf 'PASS: exact-head archive build rejects unsafe output paths, live checkout, and environment contamination\n'
