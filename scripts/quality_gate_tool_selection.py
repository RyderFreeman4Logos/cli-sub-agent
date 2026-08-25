"""Resolve sandbox tools without ambient Mise dispatchers."""

from __future__ import annotations

import os
import shutil
import stat
import sys
from pathlib import Path
from typing import Mapping

from quality_gate_host_attestation import GIT, IsolationError
from quality_gate_toolchain import (
    PinnedRustToolchain,
    resolve_pinned_nextest,
    resolve_pinned_rust_tools,
)

__all__ = ("selected_tools",)

_REQUIRED_TOOLS = ("bash", "git", "python3")
_OPTIONAL_TOOLS = (
    "ar",
    "cargo-deny",
    "cargo-nextest",
    "cc",
    "just",
    "ld",
    "lefthook",
    "make",
    "pkg-config",
    "rg",
    "shellcheck",
    "timeout",
)
_FIXED_TOOLS = {
    "bash": Path("/usr/bin/bash"),
    "git": GIT,
    "python3": Path(sys.executable).resolve(),
    "rg": Path("/usr/bin/rg"),
}
_MISE = Path("/usr/local/bin/mise")


def _resolve_ambient_path(path: Path) -> Path:
    try:
        return path.resolve(strict=False)
    except (OSError, RuntimeError) as error:
        raise IsolationError("sandbox tool provenance invalid") from error


def selected_tools(
    repo: Path, environment: Mapping[str, str]
) -> tuple[PinnedRustToolchain, dict[str, Path]]:
    mise_shims = Path(environment.get("MISE_DATA_DIR", "/usr/local/share/mise")) / "shims"
    tool_environment = dict(environment)
    tool_environment["PATH"] = os.pathsep.join(
        entry
        for entry in environment.get("PATH", os.defpath).split(os.pathsep)
        if not (
            _resolve_ambient_path(Path(entry)) == _resolve_ambient_path(mise_shims)
            or (Path(entry).name == "shims" and Path(entry).parent.name == "mise")
        )
    )
    rust_toolchain = resolve_pinned_rust_tools(repo, tool_environment)
    selected: dict[str, Path] = {}
    search_path = tool_environment["PATH"]
    pinned_nextest = resolve_pinned_nextest(repo, tool_environment)
    for name in (*_REQUIRED_TOOLS, *_OPTIONAL_TOOLS):
        candidate = pinned_nextest if name == "cargo-nextest" else _FIXED_TOOLS.get(name)
        if candidate is None and name != "cargo-nextest":
            located = shutil.which(name, path=search_path)
            if located:
                candidate = Path(located)
        if candidate is None or not candidate.exists():
            if name in _REQUIRED_TOOLS:
                raise IsolationError("required sandbox tool unavailable")
            continue
        try:
            resolved = candidate.resolve(strict=True)
            status = resolved.stat()
        except OSError as error:
            raise IsolationError("sandbox tool provenance invalid") from error
        if resolved == _MISE:
            continue
        if not stat.S_ISREG(status.st_mode) or not os.access(resolved, os.X_OK):
            raise IsolationError("sandbox tool provenance invalid")
        selected[name] = resolved
    return rust_toolchain, selected
