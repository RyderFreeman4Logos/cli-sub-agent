#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
builder="${repo_root}/scripts/build-exact-head-binaries.sh"
tmp="$(mktemp -d)"
cleanup() {
  rm -rf "${tmp}"
}
trap cleanup EXIT

fixture="${tmp}/fixture"
output="${tmp}/output"
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

[[bin]]
name = "csa"
path = "src/main.rs"
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
chmod +x "${fixture}/scripts/cargo-env-normalize.sh"
env -u RUSTC_WRAPPER \
  -u RUSTC_WORKSPACE_WRAPPER \
  -u RUSTFLAGS \
  -u CARGO_ENCODED_RUSTFLAGS \
  -u RUSTC_BOOTSTRAP \
  /usr/local/bin/cargo generate-lockfile --manifest-path "${fixture}/Cargo.toml"
git -C "${fixture}" add .
git -C "${fixture}" commit -qm "fixture"
head="$(git -C "${fixture}" rev-parse HEAD)"

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
printf 'PASS: exact-head archive build rejects live checkout and environment contamination\n'
