#!/bin/bash
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

usage() {
  cat <<'EOF'
Usage: build-exact-head-binaries.sh --repo <path> --head <commit> --output-dir <path>

Build csa and weave from a git-archive snapshot of one commit. The build runs
with an isolated Cargo home/target directory and a whitelist environment so
live-checkout ignored files, dotenv values, Cargo wrappers, and Rust flags
cannot affect the produced binaries.
EOF
}

repo=""
head=""
output_dir=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    --head)
      head="${2:-}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "${repo}" ] || [ -z "${head}" ] || [ -z "${output_dir}" ]; then
  usage >&2
  exit 2
fi
repo="$(git -C "${repo}" rev-parse --show-toplevel)"
head="$(git -C "${repo}" rev-parse --verify "${head}^{commit}")"
cargo_bin="$("${repo}/scripts/resolve-trusted-cargo.sh" --repo "${repo}")"
cargo_bin_dir="$(dirname "${cargo_bin}")"
case "${output_dir}" in
  /*) ;;
  *) output_dir="${repo}/${output_dir}" ;;
esac

scratch_parent="${TMPDIR:-/tmp}"
mkdir -p "${scratch_parent}"
scratch="$(mktemp -d "${scratch_parent%/}/csa-exact-build.XXXXXX")"
staged_output=""
cleanup() {
  rm -rf "${scratch}"
  if [ -n "${staged_output}" ]; then
    rm -rf "${staged_output}"
  fi
}
trap cleanup EXIT

checkout="${scratch}/checkout"
cargo_home="${scratch}/cargo-home"
target_dir="${scratch}/target"
mkdir -p "${checkout}" "${cargo_home}" "${target_dir}"
git -C "${repo}" archive --format=tar "${head}" | tar -xf - -C "${checkout}"

cargo_args=("${cargo_bin}")
# Cargo prefers the legacy extensionless file when both names exist.
if [ -f "${checkout}/.cargo/config" ]; then
  cargo_args+=(--config "${checkout}/.cargo/config")
elif [ -f "${checkout}/.cargo/config.toml" ]; then
  cargo_args+=(--config "${checkout}/.cargo/config.toml")
fi
cargo_args+=(
  build
  --manifest-path "${checkout}/Cargo.toml"
  --release
  --locked
  -p cli-sub-agent
  -p weave
)

# Do not inherit dotenv/Cargo/Rust build controls from the live checkout.
# The resolver selects Cargo only from fixed mise/rustup/system locations. Its
# directory leads PATH so rustup shims can find the matching rustc without
# accepting a PATH injected by a local .env file.
clean_env=(
  env -i
  "HOME=${HOME}"
  "USER=${USER:-}"
  "LOGNAME=${LOGNAME:-${USER:-}}"
  "PATH=${cargo_bin_dir}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
  "CARGO_HOME=${cargo_home}"
  "CARGO_TARGET_DIR=${target_dir}"
  "CSA_PRESERVE_CARGO_TARGET_DIR=1"
  "MISE_TRUSTED_CONFIG_PATHS=${checkout}"
  "NEXTEST_DOUBLE_SPAWN=0"
)
if [ -f "${checkout}/rust-toolchain.toml" ]; then
  rustup_toolchain="$(
    sed -nE 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' \
      "${checkout}/rust-toolchain.toml" | head -n 1
  )"
  if [ -n "${rustup_toolchain}" ]; then
    clean_env+=("RUSTUP_TOOLCHAIN=${rustup_toolchain}")
  fi
fi
if [ -n "${CARGO_BUILD_JOBS:-}" ]; then
  clean_env+=("CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}")
fi
for proxy_var in HTTP_PROXY HTTPS_PROXY NO_PROXY http_proxy https_proxy no_proxy; do
  if [ -n "${!proxy_var:-}" ]; then
    clean_env+=("${proxy_var}=${!proxy_var}")
  fi
done

(
  cd "${checkout}"
  "${clean_env[@]}" \
    "${checkout}/scripts/cargo-env-normalize.sh" \
    /bin/sh -c 'cd / && exec "$@"' csa-exact-build "${cargo_args[@]}"
)

for binary in csa weave; do
  if [ ! -x "${target_dir}/release/${binary}" ]; then
    echo "ERROR: exact-head build did not produce ${binary}." >&2
    exit 1
  fi
done

output_parent="$(dirname "${output_dir}")"
mkdir -p "${output_parent}"
staged_output="$(mktemp -d "${output_parent}/.exact-binaries.XXXXXX")"
install -m 0755 "${target_dir}/release/csa" "${staged_output}/csa"
install -m 0755 "${target_dir}/release/weave" "${staged_output}/weave"
printf '%s\n' "${head}" >"${staged_output}/SOURCE_COMMIT"
rm -rf "${output_dir}"
mv "${staged_output}" "${output_dir}"
staged_output=""
printf 'Built exact-head binaries from %s at %s\n' "${head}" "${output_dir}"
