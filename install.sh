#!/bin/sh
# install.sh - Install csa (cli-sub-agent) and weave
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --from-source
#   curl -fsSL .../install.sh | sh -s -- --help
#
# Environment variables:
#   CSA_INSTALL_DIR  Override install directory (default: ~/.local/bin)

set -e

REPO_OWNER="RyderFreeman4Logos"
REPO_NAME="cli-sub-agent"
GITHUB_REPO="${REPO_OWNER}/${REPO_NAME}"
GITHUB_API="https://api.github.com/repos/${GITHUB_REPO}/releases/latest"
GITHUB_GIT="https://github.com/${GITHUB_REPO}.git"
INSTALL_DIR="${CSA_INSTALL_DIR:-${HOME}/.local/bin}"
TMPDIR_BASE="${TMPDIR:-/tmp}"
CLEANUP_DIR=""

# --- Output helpers ---

info() {
    printf '  \033[1;34m==>\033[0m %s\n' "$1"
}

success() {
    printf '  \033[1;32m==>\033[0m %s\n' "$1"
}

warn() {
    printf '  \033[1;33mWARN:\033[0m %s\n' "$1" >&2
}

error() {
    printf '  \033[1;31mERROR:\033[0m %s\n' "$1" >&2
    exit 1
}

# --- Cleanup on exit ---

cleanup() {
    if [ -n "${CLEANUP_DIR}" ] && [ -d "${CLEANUP_DIR}" ]; then
        rm -rf "${CLEANUP_DIR}"
    fi
}

trap cleanup EXIT INT TERM

# --- HTTP download abstraction ---

# download URL DEST
# Downloads URL to DEST file. Tries curl first, falls back to wget.
download() {
    _url="$1"
    _dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "${_dest}" "${_url}"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "${_dest}" "${_url}"
    else
        error "Neither curl nor wget found. Please install one and retry."
    fi
}

# download_stdout URL
# Downloads URL content to stdout. Tries curl first, falls back to wget.
download_stdout() {
    _url="$1"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "${_url}"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "${_url}"
    else
        error "Neither curl nor wget found. Please install one and retry."
    fi
}

# --- Help ---

usage() {
    cat <<'HELP'
install.sh - Install csa (cli-sub-agent) and weave

USAGE:
    sh install.sh [OPTIONS]

OPTIONS:
    --from-source    Compile from source instead of downloading prebuilt binaries
    --help, -h       Show this help message

ENVIRONMENT:
    CSA_INSTALL_DIR  Override install directory (default: ~/.local/bin)

MODES:
    Default          Download prebuilt binaries from GitHub Releases
    --from-source    Clone and compile with cargo (installs Rust via mise if needed)

EXAMPLES:
    # Download prebuilt binary
    curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh

    # Compile from source
    curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh -s -- --from-source
HELP
    exit 0
}

# --- Detect platform ---

detect_platform() {
    _os="$(uname -s)"
    _arch="$(uname -m)"

    case "${_os}" in
        Linux)  OS="linux" ;;
        Darwin) OS="darwin" ;;
        *)      error "Unsupported OS: ${_os}. Only Linux and macOS are supported." ;;
    esac

    case "${_arch}" in
        x86_64|amd64)       ARCH="x86_64" ;;
        aarch64|arm64)      ARCH="aarch64" ;;
        *)                  error "Unsupported architecture: ${_arch}. Only x86_64 and aarch64/arm64 are supported." ;;
    esac

    # Map to Rust target triple
    case "${OS}-${ARCH}" in
        linux-x86_64)   TARGET="x86_64-unknown-linux-musl" ;;
        linux-aarch64)  TARGET="aarch64-unknown-linux-musl" ;;
        darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
        darwin-aarch64) TARGET="aarch64-apple-darwin" ;;
        *)              error "Unsupported platform: ${OS}-${ARCH}" ;;
    esac

    info "Detected platform: ${OS} ${ARCH} (${TARGET})"
}

# --- Fetch latest release tag ---

fetch_latest_tag() {
    info "Fetching latest release tag from GitHub..."
    _response_file="${CLEANUP_DIR}/api_response.json"
    download "${GITHUB_API}" "${_response_file}"

    # Extract tag_name from JSON without jq (POSIX-compatible)
    TAG=""
    while IFS= read -r line; do
        case "${line}" in
            *'"tag_name"'*)
                TAG="$(printf '%s' "${line}" | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
                break
                ;;
        esac
    done < "${_response_file}"

    if [ -z "${TAG}" ]; then
        error "Failed to determine latest release tag. Check network connectivity or visit https://github.com/${GITHUB_REPO}/releases"
    fi

    info "Latest release: ${TAG}"
}

# --- Install from prebuilt binary ---

install_binary() {
    detect_platform

    CLEANUP_DIR="$(mktemp -d "${TMPDIR_BASE}/csa-install.XXXXXX")"

    fetch_latest_tag

    ARCHIVE_NAME="csa-${TARGET}.tar.gz"
    DOWNLOAD_URL="https://github.com/${GITHUB_REPO}/releases/download/${TAG}/${ARCHIVE_NAME}"

    info "Downloading ${ARCHIVE_NAME}..."
    download "${DOWNLOAD_URL}" "${CLEANUP_DIR}/${ARCHIVE_NAME}"

    info "Extracting archive..."
    tar -xzf "${CLEANUP_DIR}/${ARCHIVE_NAME}" -C "${CLEANUP_DIR}"

    # Ensure install directory exists
    mkdir -p "${INSTALL_DIR}"

    # Install binaries
    for _bin in csa weave; do
        if [ -f "${CLEANUP_DIR}/${_bin}" ]; then
            cp "${CLEANUP_DIR}/${_bin}" "${INSTALL_DIR}/${_bin}"
            chmod +x "${INSTALL_DIR}/${_bin}"
            success "Installed ${_bin} to ${INSTALL_DIR}/${_bin}"
        else
            warn "${_bin} binary not found in archive, skipping"
        fi
    done

    verify_install
    print_path_hint
    print_next_steps
}

# --- Install from source ---

install_source() {
    info "Installing from source..."

    # Check for cargo
    if ! command -v cargo >/dev/null 2>&1; then
        info "cargo not found, attempting to install Rust via mise..."

        # Check/install mise
        if ! command -v mise >/dev/null 2>&1; then
            info "Installing mise..."
            download_stdout "https://mise.run" | sh

            # Add mise to PATH for current session
            if [ -f "${HOME}/.local/bin/mise" ]; then
                export PATH="${HOME}/.local/bin:${PATH}"
            fi
        fi

        if command -v mise >/dev/null 2>&1; then
            info "Installing Rust toolchain via mise..."
            mise use -g rust

            # Add cargo bin to PATH for current session
            if [ -d "${HOME}/.local/share/mise/installs/rust/latest/bin" ]; then
                export PATH="${HOME}/.local/share/mise/installs/rust/latest/bin:${PATH}"
            fi
        fi

        # Final check
        if ! command -v cargo >/dev/null 2>&1; then
            error "Could not install cargo automatically.
Please install Rust manually: https://rustup.rs
Then re-run this script with --from-source"
        fi
    fi

    success "Found cargo: $(cargo --version)"

    info "Compiling csa (this may take a few minutes)..."
    cargo install --git "${GITHUB_GIT}" cli-sub-agent

    info "Compiling weave..."
    cargo install --git "${GITHUB_GIT}" weave

    verify_install
    print_next_steps
}

# --- Verification ---

verify_install() {
    info "Verifying installation..."

    _ok=true
    if command -v csa >/dev/null 2>&1; then
        _ver="$(csa --version 2>/dev/null || echo '(version unknown)')"
        success "csa ${_ver}"
    elif [ -x "${INSTALL_DIR}/csa" ]; then
        success "csa installed at ${INSTALL_DIR}/csa (not yet in PATH)"
    else
        warn "csa binary not found"
        _ok=false
    fi

    if command -v weave >/dev/null 2>&1; then
        _ver="$(weave --version 2>/dev/null || echo '(version unknown)')"
        success "weave ${_ver}"
    elif [ -x "${INSTALL_DIR}/weave" ]; then
        success "weave installed at ${INSTALL_DIR}/weave (not yet in PATH)"
    else
        warn "weave binary not found"
        _ok=false
    fi

    if [ "${_ok}" = true ]; then
        echo ""
        success "Installation complete!"
    else
        echo ""
        warn "Some binaries could not be verified. Check the output above."
    fi
}

# --- PATH hint ---

print_path_hint() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*)
            # Already in PATH, no hint needed
            ;;
        *)
            echo ""
            warn "${INSTALL_DIR} is not in your PATH."
            echo "  Add it by appending one of these to your shell profile:"
            echo ""
            echo "    # bash (~/.bashrc or ~/.bash_profile):"
            echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            echo "    # zsh (~/.zshrc):"
            echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            echo "    # fish (~/.config/fish/config.fish):"
            echo "    fish_add_path ${INSTALL_DIR}"
            echo ""
            echo "  Then restart your shell or run:"
            echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

# --- Next steps ---

print_next_steps() {
    echo ""
    echo "  Next steps:"
    echo "    1. Initialize a project:  csa init"
    echo "    2. Check tool status:     csa doctor"
    echo "    3. Run your first task:   csa run \"hello world\""
    echo ""
    echo "  Documentation: https://github.com/${GITHUB_REPO}"
    echo ""
}

# --- Main ---

main() {
    FROM_SOURCE=false

    for arg in "$@"; do
        case "${arg}" in
            --from-source)  FROM_SOURCE=true ;;
            --help|-h)      usage ;;
            *)              error "Unknown option: ${arg}. Use --help for usage." ;;
        esac
    done

    echo ""
    echo "  Installing csa (cli-sub-agent)"
    echo "  ==============================="
    echo ""

    if [ "${FROM_SOURCE}" = true ]; then
        install_source
    else
        install_binary
    fi
}

main "$@"
