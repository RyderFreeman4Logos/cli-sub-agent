#!/usr/bin/env bash
# Update Homebrew formula with SHA256 hashes from a GitHub release.
# Usage: ./homebrew/update-formula.sh v0.1.0
set -euo pipefail

VERSION="${1:?Usage: $0 <version-tag>}"
VERSION_NUM="${VERSION#v}"
REPO="RyderFreeman4Logos/cli-sub-agent"
FORMULA="homebrew/csa.rb"

TARGETS=(
    "aarch64-apple-darwin:PLACEHOLDER_SHA256_ARM64_DARWIN"
    "x86_64-apple-darwin:PLACEHOLDER_SHA256_X86_64_DARWIN"
    "aarch64-unknown-linux-gnu:PLACEHOLDER_SHA256_ARM64_LINUX"
    "x86_64-unknown-linux-gnu:PLACEHOLDER_SHA256_X86_64_LINUX"
)

echo "Updating formula for ${VERSION}..."

# Update version
sed -i "s/version \".*\"/version \"${VERSION_NUM}\"/" "${FORMULA}"

for entry in "${TARGETS[@]}"; do
    target="${entry%%:*}"
    placeholder="${entry#*:}"
    url="https://github.com/${REPO}/releases/download/${VERSION}/csa-${target}.tar.gz"

    echo "  Fetching SHA256 for ${target}..."
    sha256=$(curl -sL "${url}" | sha256sum | cut -d' ' -f1)

    if [ -z "${sha256}" ] || [ "${sha256}" = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" ]; then
        echo "  WARNING: Empty or missing asset for ${target}, skipping"
        continue
    fi

    sed -i "s/${placeholder}/${sha256}/" "${FORMULA}"
    echo "  ${target}: ${sha256}"
done

echo "Formula updated: ${FORMULA}"
