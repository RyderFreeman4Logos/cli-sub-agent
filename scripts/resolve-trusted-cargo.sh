#!/bin/bash
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

repo=""
home_only=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    --home-only)
      home_only=1
      shift
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done
if [ -z "${repo}" ]; then
  echo "Usage: resolve-trusted-cargo.sh --repo <path> [--home-only]" >&2
  exit 2
fi
repo="$(git -C "${repo}" rev-parse --show-toplevel)"

candidates=()
if [ "${home_only}" = "0" ]; then
  for mise_bin in /usr/local/bin/mise /opt/homebrew/bin/mise "${HOME}/.local/bin/mise"; do
    if [ -x "${mise_bin}" ]; then
      resolved="$(MISE_TRUSTED_CONFIG_PATHS="${repo}" "${mise_bin}" which cargo 2>/dev/null || true)"
      case "${resolved}" in
        /*) candidates+=("${resolved}") ;;
      esac
    fi
  done
fi
candidates+=(
  "${HOME}/.cargo/bin/cargo"
  "${HOME}/.local/share/mise/shims/cargo"
  "${HOME}/.local/bin/cargo"
)
if [ "${home_only}" = "0" ]; then
  candidates+=(
    /opt/homebrew/bin/cargo
    /usr/local/bin/cargo
    /usr/bin/cargo
  )
fi

for candidate in "${candidates[@]}"; do
  case "${candidate}" in
    /*) ;;
    *) continue ;;
  esac
  if [ ! -x "${candidate}" ]; then
    continue
  fi
  candidate_dir="$(dirname "${candidate}")"
  if env -i \
    "HOME=${HOME}" \
    "USER=${USER:-}" \
    "PATH=${candidate_dir}:/usr/local/bin:/usr/bin:/bin" \
    "MISE_TRUSTED_CONFIG_PATHS=${repo}" \
    "${candidate}" --version >/dev/null 2>&1; then
    printf '%s\n' "${candidate}"
    exit 0
  fi
done

echo "ERROR: no trusted Cargo executable found in fixed mise, rustup, Homebrew, or system locations." >&2
exit 1
