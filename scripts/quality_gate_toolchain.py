"""Resolve the repository-pinned Rust toolchain without ambient proxies.

The quality-gate sandbox consumes only verified executable realpaths from one
complete, already-installed toolchain.  Resolution happens before namespace
entry so the sandbox never needs rustup metadata or network capability.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tomllib
from pathlib import Path
from typing import Mapping, Sequence

__all__ = (
    "ToolchainError",
    "resolve_pinned_rust_tools",
)

_RUST_TOOLS = (
    "cargo",
    "cargo-clippy",
    "cargo-fmt",
    "clippy-driver",
    "rustc",
    "rustdoc",
    "rustfmt",
)


class ToolchainError(RuntimeError):
    """The pinned local Rust toolchain is incomplete or ambiguous."""

    def __init__(self, reason: str) -> None:
        super().__init__("pinned Rust toolchain unavailable")
        self.reason = reason


def _validated_executable(candidate: Path, reason: str) -> Path:
    try:
        resolved = candidate.resolve(strict=True)
        status = resolved.stat()
    except OSError as error:
        raise ToolchainError(reason) from error
    if not stat.S_ISREG(status.st_mode) or not os.access(resolved, os.X_OK):
        raise ToolchainError(reason)
    return resolved


def _run_query(
    command: Sequence[str], repo: Path, environment: Mapping[str, str], reason: str
) -> bytes:
    query_environment = {
        "HOME": environment.get("HOME", "/"),
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": environment.get("PATH", os.defpath),
    }
    rustup_home = environment.get("RUSTUP_HOME")
    if rustup_home:
        query_environment["RUSTUP_HOME"] = rustup_home
    try:
        completed = subprocess.run(
            command,
            cwd=repo,
            env=query_environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ToolchainError(reason) from error
    if completed.returncode != 0 or len(completed.stdout) > 4096:
        raise ToolchainError(reason)
    return completed.stdout


def _rustup_which(
    rustup: Path,
    selector: str,
    tool: str,
    repo: Path,
    environment: Mapping[str, str],
) -> Path:
    output = _run_query(
        (str(rustup), "which", "--toolchain", selector, tool),
        repo,
        environment,
        "toolchain_component_missing",
    )
    try:
        lines = output.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ToolchainError("toolchain_invalid") from error
    if len(lines) != 1 or not os.path.isabs(lines[0]):
        raise ToolchainError("toolchain_invalid")
    return _validated_executable(Path(lines[0]), "toolchain_component_missing")


def resolve_pinned_rust_tools(
    repo: Path, environment: Mapping[str, str]
) -> tuple[str, dict[str, Path]]:
    """Return an exact selector and verified tools from its canonical sysroot.

    The pin must be a concrete version with the Clippy and rustfmt components.
    Every required executable must resolve through ``rustup which`` to the same
    installed sysroot.  Missing or ambiguous inputs fail closed with a stable
    reason suitable for the quality-gate structured result.
    """

    try:
        configuration = tomllib.loads(
            (repo / "rust-toolchain.toml").read_text(encoding="utf-8")
        )
        toolchain = configuration["toolchain"]
        channel = toolchain["channel"]
        components = toolchain["components"]
    except (
        OSError,
        UnicodeError,
        tomllib.TOMLDecodeError,
        KeyError,
        TypeError,
    ) as error:
        raise ToolchainError("toolchain_invalid") from error
    if (
        not isinstance(channel, str)
        or not channel
        or not channel[0].isdigit()
        or any(not (character.isalnum() or character in ".-_") for character in channel)
    ):
        raise ToolchainError("toolchain_invalid")
    if not isinstance(components, list) or any(
        not isinstance(component, str) for component in components
    ):
        raise ToolchainError("toolchain_invalid")
    if not {"clippy", "rustfmt"}.issubset(components):
        raise ToolchainError("toolchain_component_missing")

    rustup_value = shutil.which("rustup", path=environment.get("PATH"))
    if not rustup_value:
        raise ToolchainError("toolchain_component_missing")
    rustup = _validated_executable(Path(rustup_value), "toolchain_component_missing")
    selected_rustc = _rustup_which(rustup, channel, "rustc", repo, environment)
    version = _run_query(
        (str(selected_rustc), "-vV"), repo, environment, "toolchain_invalid"
    )
    version_fields: dict[bytes, list[bytes]] = {}
    for line in version.splitlines():
        if b":" not in line:
            continue
        key, value = line.split(b":", 1)
        version_fields.setdefault(key, []).append(value.strip())
    hosts = version_fields.get(b"host", [])
    releases = version_fields.get(b"release", [])
    if len(hosts) != 1 or len(releases) != 1:
        raise ToolchainError("toolchain_invalid")
    try:
        host = hosts[0].decode("ascii")
        release = releases[0].decode("ascii")
    except UnicodeDecodeError as error:
        raise ToolchainError("toolchain_invalid") from error
    if not host or release != channel:
        raise ToolchainError("toolchain_invalid")
    selector = f"{channel}-{host}"
    exact_rustc = _rustup_which(rustup, selector, "rustc", repo, environment)
    if exact_rustc != selected_rustc:
        raise ToolchainError("toolchain_invalid")

    sysroot_output = _run_query(
        (str(exact_rustc), "--print", "sysroot"),
        repo,
        environment,
        "toolchain_invalid",
    )
    try:
        sysroot_lines = sysroot_output.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ToolchainError("toolchain_invalid") from error
    if len(sysroot_lines) != 1 or not os.path.isabs(sysroot_lines[0]):
        raise ToolchainError("toolchain_invalid")
    try:
        sysroot = Path(sysroot_lines[0]).resolve(strict=True)
        rust_bin = (sysroot / "bin").resolve(strict=True)
    except OSError as error:
        raise ToolchainError("toolchain_invalid") from error
    if exact_rustc.parent != rust_bin:
        raise ToolchainError("toolchain_invalid")

    selected = {"rustc": exact_rustc}
    for name in _RUST_TOOLS:
        if name == "rustc":
            continue
        executable = _rustup_which(rustup, selector, name, repo, environment)
        if executable.parent != rust_bin:
            raise ToolchainError("toolchain_invalid")
        selected[name] = executable
    return selector, selected
