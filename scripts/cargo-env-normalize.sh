#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

ensure_writable_dir() {
    local path="$1"
    mkdir -p "$path" 2>/dev/null || true
    dir_writable "$path"
}

dir_writable() {
    local path="$1"
    [ -d "$path" ] || return 1
    local probe="${path}/.csa-write-probe.$$"
    if touch "$probe" >/dev/null 2>&1; then
        rm -f "$probe"
        return 0
    fi
    return 1
}

rust_state_needs_override() {
    local value="${1:-}"
    if [ -z "$value" ] || [ "$value" = "/usr/local" ]; then
        return 0
    fi
    case "$value" in
        /usr/local/*)
            dir_writable "$value" && return 1
            return 0
            ;;
    esac
    return 1
}

if rust_state_needs_override "${CARGO_HOME:-}"; then
    if ensure_writable_dir "/usr/local/share/cargo"; then
        export CARGO_HOME="/usr/local/share/cargo"
    elif [ -n "${HOME:-}" ] && ensure_writable_dir "${HOME}/.cargo"; then
        export CARGO_HOME="${HOME}/.cargo"
    else
        echo "error: no writable Cargo home available; refusing to create repo-local .cargo-local fallback" >&2
        echo "hint: make /usr/local/share/cargo or HOME/.cargo writable, or set CARGO_HOME to an explicit writable shared cache" >&2
        exit 1
    fi
fi

if rust_state_needs_override "${CARGO_INSTALL_ROOT:-}"; then
    export CARGO_INSTALL_ROOT="${repo_root}/target/cargo-install-root"
    mkdir -p "$CARGO_INSTALL_ROOT"
fi

export CARGO_TARGET_DIR="${repo_root}/target"

mise_rust_home="${MISE_DATA_DIR:-/usr/local/share/mise}/installs/rust/stable"
if rust_state_needs_override "${RUSTUP_HOME:-}" \
    && [ -f "${mise_rust_home}/settings.toml" ] \
    && [ -d "${mise_rust_home}/toolchains" ]; then
    export RUSTUP_HOME="$mise_rust_home"
elif rust_state_needs_override "${RUSTUP_HOME:-}"; then
    export RUSTUP_HOME="${HOME:-${repo_root}}/.rustup"
    mkdir -p "$RUSTUP_HOME"
fi

if [ -f "${repo_root}/rust-toolchain.toml" ] && [ -d "${mise_rust_home}/toolchains" ]; then
    channel="$(
        sed -nE 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' \
            "${repo_root}/rust-toolchain.toml" \
            | head -n 1
    )"
    if [ -n "$channel" ]; then
        for toolchain in "${mise_rust_home}/toolchains/${channel}"-* "${mise_rust_home}/toolchains/${channel}"; do
            if [ -x "${toolchain}/bin/cargo" ]; then
                case ":${PATH}:" in
                    *":${toolchain}/bin:"*) ;;
                    *) export PATH="${toolchain}/bin:${PATH}" ;;
                esac
                break
            fi
        done
    fi
fi

exec "$@"
