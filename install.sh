#!/usr/bin/env bash
set -euo pipefail

# CSA (CLI Sub-Agent) Installer
# Supports: macOS, Linux
# Installs or updates rustup, then installs csa via cargo

REPO_URL="https://github.com/RyderFreeman4Logos/cli-sub-agent"
PACKAGE="cli-sub-agent"

main() {
    echo "=== CSA (CLI Sub-Agent) Installer ==="
    echo ""

    # Check platform
    case "$(uname -s)" in
        Linux)  echo "Platform: Linux" ;;
        Darwin) echo "Platform: macOS" ;;
        *)
            echo "Error: Unsupported platform '$(uname -s)'."
            echo "Only macOS and Linux are supported."
            exit 1
            ;;
    esac

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

    # Verify cargo
    if ! command -v cargo &>/dev/null; then
        echo "Error: cargo not found after rustup installation."
        echo "Try running: source \"\${CARGO_HOME:-\$HOME/.cargo}/env\""
        exit 1
    fi
    echo "Cargo: $(cargo --version)"

    # Install csa
    echo ""
    echo "Installing csa from ${REPO_URL}..."
    cargo install --git "${REPO_URL}" -p "${PACKAGE}" --all-features --locked

    # Verify installation
    echo ""
    echo "=== Installation complete ==="
    if command -v csa &>/dev/null; then
        echo "Installed: $(csa --version)"
        echo ""
        echo "Quick start:"
        echo "  cd your-project"
        echo "  csa init                              # Initialize config"
        echo "  csa run --tool gemini-cli \"prompt\"     # Run a task"
        echo ""
        echo "See: https://github.com/RyderFreeman4Logos/cli-sub-agent"
    else
        echo "Warning: 'csa' not found in PATH."
        echo "Ensure ~/.cargo/bin is in your PATH:"
        echo "  export PATH=\"\$HOME/.cargo/bin:\$PATH\""
    fi
}

main "$@"
