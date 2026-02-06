#!/usr/bin/env bash
set -euo pipefail

# CSA (CLI Sub-Agent) Installer
# Supports: macOS (Intel + Apple Silicon), Linux (amd64 + arm64)
# Tries pre-compiled binary first, falls back to cargo install

REPO="RyderFreeman4Logos/cli-sub-agent"
REPO_URL="https://github.com/${REPO}"
PACKAGE="cli-sub-agent"
BIN_NAME="csa"
INSTALL_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"

detect_target() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "${os}" in
        Linux)
            case "${arch}" in
                x86_64)  echo "x86_64-unknown-linux-gnu" ;;
                aarch64) echo "aarch64-unknown-linux-gnu" ;;
                *)       echo "unsupported" ;;
            esac
            ;;
        Darwin)
            case "${arch}" in
                x86_64)  echo "x86_64-apple-darwin" ;;
                arm64)   echo "aarch64-apple-darwin" ;;
                *)       echo "unsupported" ;;
            esac
            ;;
        *)
            echo "unsupported"
            ;;
    esac
}

get_latest_tag() {
    # Use GitHub API to get latest release tag
    if command -v curl &>/dev/null; then
        curl -sL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/'
    fi
}

try_prebuilt() {
    local target="$1"
    local tag

    tag="$(get_latest_tag)"
    if [ -z "${tag}" ]; then
        echo "No release found, will build from source."
        return 1
    fi

    local archive_name="csa-${tag}-${target}.tar.gz"
    local url="${REPO_URL}/releases/download/${tag}/${archive_name}"

    echo "Downloading ${BIN_NAME} ${tag} for ${target}..."

    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "${tmpdir}"' EXIT

    if curl -fsSL "${url}" -o "${tmpdir}/${archive_name}"; then
        tar xzf "${tmpdir}/${archive_name}" -C "${tmpdir}"
        mkdir -p "${INSTALL_DIR}"
        cp "${tmpdir}/csa-${tag}-${target}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
        chmod +x "${INSTALL_DIR}/${BIN_NAME}"
        echo "Installed pre-built binary to ${INSTALL_DIR}/${BIN_NAME}"
        return 0
    else
        echo "Pre-built binary not available for ${target}."
        return 1
    fi
}

install_from_source() {
    echo "Building from source..."

    # Install or update Rust toolchain
    if command -v rustup &>/dev/null; then
        echo "Rustup found. Updating stable toolchain..."
        rustup update stable --no-self-update
    else
        echo "Rustup not found. Installing..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
        # shellcheck source=/dev/null
        source "${CARGO_HOME:-$HOME/.cargo}/env"
    fi

    if ! command -v cargo &>/dev/null; then
        echo "Error: cargo not found after rustup installation."
        echo "Try running: source \"\${CARGO_HOME:-\$HOME/.cargo}/env\""
        exit 1
    fi
    echo "Cargo: $(cargo --version)"

    echo ""
    echo "Installing csa from ${REPO_URL}..."
    cargo install --git "${REPO_URL}" -p "${PACKAGE}" --all-features --locked
}

main() {
    echo "=== CSA (CLI Sub-Agent) Installer ==="
    echo ""

    local target
    target="$(detect_target)"

    if [ "${target}" = "unsupported" ]; then
        echo "Error: Unsupported platform '$(uname -s) $(uname -m)'."
        echo "Supported: Linux (x86_64, aarch64), macOS (x86_64, arm64)"
        exit 1
    fi

    echo "Platform: $(uname -s) $(uname -m) (${target})"
    echo ""

    # Try pre-built binary first, fall back to source
    if ! try_prebuilt "${target}"; then
        install_from_source
    fi

    # Verify installation
    echo ""
    echo "=== Installation complete ==="
    if command -v "${BIN_NAME}" &>/dev/null; then
        echo "Installed: $(${BIN_NAME} --version)"
        echo ""
        echo "Quick start:"
        echo "  cd your-project"
        echo "  csa init                              # Initialize config"
        echo "  csa run --tool gemini-cli \"prompt\"     # Run a task"
        echo ""
        echo "See: ${REPO_URL}"
    else
        echo "Warning: '${BIN_NAME}' not found in PATH."
        echo "Ensure ${INSTALL_DIR} is in your PATH:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
}

main "$@"
