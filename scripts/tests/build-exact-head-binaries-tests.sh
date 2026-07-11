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
mkdir -p "${fixture}/scripts"
git -C "${tmp}" init -q fixture
git -C "${fixture}" config user.name "Exact Build Test"
git -C "${fixture}" config user.email "exact-build@example.invalid"

cat >"${fixture}/.gitignore" <<'EOF'
.cargo/
.env
EOF
cat >"${fixture}/Cargo.toml" <<'EOF'
[workspace]
members = []
EOF
cat >"${fixture}/scripts/cargo-env-normalize.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = "cargo" ]
[ "$2" = "build" ]
[ ! -e .env ]
[ ! -e .cargo/config.toml ]
for forbidden in RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_BOOTSTRAP CARGO_PROFILE_RELEASE_OPT_LEVEL; do
  if [ -n "${!forbidden+x}" ]; then
    echo "forbidden build variable survived: ${forbidden}" >&2
    exit 1
  fi
done
case "${CARGO_HOME}" in
  "${PWD}"/*|"${HOME}/.cargo")
    echo "Cargo home is not isolated from live configuration: ${CARGO_HOME}" >&2
    exit 1
    ;;
esac
mkdir -p "${CARGO_TARGET_DIR}/release"
for binary in csa weave; do
  cat >"${CARGO_TARGET_DIR}/release/${binary}" <<BIN
#!/usr/bin/env bash
printf '%s\n' 'exact archive binary: ${binary}'
BIN
  chmod +x "${CARGO_TARGET_DIR}/release/${binary}"
done
EOF
chmod +x "${fixture}/scripts/cargo-env-normalize.sh"
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
