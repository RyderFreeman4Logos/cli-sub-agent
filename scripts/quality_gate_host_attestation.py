"""Host Git attestation for quality-gate receipt provenance.

Attests HEAD/index equality and physical tracked worktree bytes without trusting
index skip-worktree/assume-unchanged bits. Shared by the sandbox coordinator so
sanitized execution projections never stand in for host clean-state.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import tempfile
from pathlib import Path

__all__ = (
    "IsolationError",
    "host_clean_state",
    "run_git",
    "safe_git_environment",
)

GIT = Path("/usr/bin/git")


class IsolationError(RuntimeError):
    """Required sandbox preparation or containment could not be proven."""


def safe_git_environment() -> dict[str, str]:
    return {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_SYSTEM": "/dev/null",
        "GIT_OPTIONAL_LOCKS": "0",
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
    }


def run_git(repo: Path, *arguments: str) -> bytes:
    try:
        completed = subprocess.run(
            (str(GIT), *arguments),
            cwd=repo,
            env=safe_git_environment(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise IsolationError("source snapshot unavailable") from error
    if completed.returncode != 0:
        raise IsolationError("source snapshot unavailable")
    return completed.stdout


def _run_private_index_git(
    repo: Path, index: Path, *arguments: str, input_bytes: bytes | None = None
) -> subprocess.CompletedProcess[bytes]:
    environment = safe_git_environment()
    environment.update(
        {
            "GIT_INDEX_FILE": os.fspath(index),
            "GIT_OPTIONAL_LOCKS": "0",
        }
    )
    command = (
        str(GIT),
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.ignoreStat=false",
        "-c",
        "core.sparseCheckout=false",
        "-c",
        "index.sparse=false",
        "-c",
        "core.fileMode=true",
        *arguments,
    )
    try:
        return subprocess.run(
            command,
            cwd=repo,
            env=environment,
            input=input_bytes,
            stdin=subprocess.DEVNULL if input_bytes is None else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise IsolationError("source clean-state unavailable") from error


def host_clean_state(repo: Path) -> tuple[str, str, str, str]:
    """Attest HEAD/index equality and physical tracked bytes without index trust bits."""

    head_tree = run_git(repo, "rev-parse", "HEAD^{tree}").strip()
    index_records = run_git(repo, "ls-files", "--stage", "-z")
    with tempfile.TemporaryDirectory(prefix="csa-quality-gate-index.") as owner:
        logical_index = Path(owner) / "logical-index"
        initialized = _run_private_index_git(
            repo, logical_index, "read-tree", "--empty"
        )
        populated = _run_private_index_git(
            repo,
            logical_index,
            "update-index",
            "-z",
            "--index-info",
            input_bytes=index_records,
        )
        written = _run_private_index_git(repo, logical_index, "write-tree")
        if any(
            result.returncode != 0 for result in (initialized, populated, written)
        ):
            raise IsolationError("source clean-state unavailable")
        index_tree = written.stdout.strip()
        if not index_tree:
            raise IsolationError("source clean-state unavailable")
        index_clean = str(index_tree == head_tree).lower()
        private_index = Path(owner) / "physical-index"
        loaded = _run_private_index_git(
            repo,
            private_index,
            "read-tree",
            "--no-sparse-checkout",
            os.fsdecode(index_tree),
        )
        if loaded.returncode != 0:
            raise IsolationError("source clean-state unavailable")
        refreshed = _run_private_index_git(
            repo,
            private_index,
            "update-index",
            "--really-refresh",
            "-q",
            "--ignore-submodules",
            "--",
        )
        compared = _run_private_index_git(
            repo,
            private_index,
            "diff-files",
            "--quiet",
            "--ignore-submodules",
            "--",
        )
        if refreshed.returncode not in {0, 1} or compared.returncode not in {0, 1}:
            raise IsolationError("source clean-state unavailable")
        if refreshed.returncode == 1 and compared.returncode == 0:
            raise IsolationError("source clean-state unavailable")
        tracked_clean = str(
            refreshed.returncode == 0 and compared.returncode == 0
        ).lower()
    untracked = run_git(repo, "ls-files", "--others", "--exclude-standard", "-z")
    return (
        index_clean,
        tracked_clean,
        hashlib.sha256(untracked).hexdigest(),
        os.fsdecode(index_tree),
    )
