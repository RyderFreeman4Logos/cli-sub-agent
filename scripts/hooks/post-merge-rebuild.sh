#!/usr/bin/env bash
# Post-merge: rebuild and install csa binary after pulling new code.
# Skips gracefully in sandboxed/CI environments where build is not possible.

# Skip inside CSA sessions (sandboxed, filesystem may be read-only)
if [ -n "${CSA_SESSION_ID:-}" ]; then
    echo "[post-merge] Inside CSA session — skipping rebuild (sandbox)."
    exit 0
fi

# Skip if install target is not writable
if [ ! -w /usr/local/bin ]; then
    echo "[post-merge] /usr/local/bin is not writable — skipping rebuild."
    exit 0
fi

echo "[post-merge] Rebuilding csa..."
if just install; then
    echo "[post-merge] csa installed successfully."
else
    echo "[post-merge] WARNING: just install failed (exit $?). csa binary may be stale." >&2
fi
