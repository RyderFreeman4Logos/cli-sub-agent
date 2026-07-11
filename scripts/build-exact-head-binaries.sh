#!/usr/bin/env bash
set -euo pipefail

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

# Do not inherit dotenv/Cargo/Rust build controls from the live checkout.
# The fixed PATH preserves the repository's configured system/mise toolchain
# without accepting a PATH injected by a local .env file.
clean_env=(
  env -i
  "HOME=${HOME}"
  "USER=${USER:-}"
  "LOGNAME=${LOGNAME:-${USER:-}}"
  "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
  "CARGO_HOME=${cargo_home}"
  "CARGO_TARGET_DIR=${target_dir}"
  "MISE_TRUSTED_CONFIG_PATHS=${checkout}"
  "NEXTEST_DOUBLE_SPAWN=0"
)
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
    "${checkout}/scripts/cargo-env-normalize.sh" cargo build \
    --release --locked -p cli-sub-agent -p weave
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
