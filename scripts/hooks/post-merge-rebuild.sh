#!/usr/bin/env bash
# Post-merge: rebuild and install csa binary after pulling new code.
# Ordering contract: install + active PATH provenance verify -> cargo clean.
# Never clean before successful installation/provenance verification.
# Skips gracefully in sandboxed/CI environments where build is not possible.

set -u

# Skip inside CSA sessions (sandboxed, filesystem may be read-only)
if [ -n "${CSA_SESSION_ID:-}" ]; then
    echo "[post-merge] Inside CSA session — skipping rebuild (sandbox)."
    exit 0
fi

INSTALL_DIR="${CSA_POST_MERGE_INSTALL_DIR:-/usr/local/bin}"
JUST_CMD="${CSA_POST_MERGE_JUST:-just}"
CARGO_CMD="${CSA_POST_MERGE_CARGO:-cargo}"

# Skip if install target is not writable
if [ ! -w "$INSTALL_DIR" ]; then
    echo "[post-merge] $INSTALL_DIR is not writable — skipping rebuild."
    exit 0
fi

echo "[post-merge] Rebuilding csa..."
if "$JUST_CMD" install; then
    echo "[post-merge] csa active-binary provenance verified."
    echo "[post-merge] Cleaning cargo target..."
    if "$CARGO_CMD" clean; then
        echo "[post-merge] cargo clean completed."
        echo "[post-merge] Post-merge rebuild finished successfully."
    else
        clean_rc=$?
        echo "[post-merge] WARNING: cargo clean failed (exit ${clean_rc}). Install/provenance succeeded but target/ was not cleaned; post-merge completion is partial." >&2
    fi
else
    install_rc=$?
    echo "[post-merge] WARNING: just install failed (exit ${install_rc}). csa binary may be stale; skipping cargo clean." >&2
fi
