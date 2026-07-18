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
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

__all__ = (
    "PinnedRustToolchain",
    "SANDBOX_RUST_TOOLCHAIN_ROOT",
    "ToolchainError",
    "resolve_pinned_rust_tools",
)

SANDBOX_RUST_TOOLCHAIN_ROOT = Path("/run/csa-rust-toolchain")

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


@dataclass(frozen=True)
class PinnedRustToolchain:
    """Canonical installed sysroot and every executable required by the gate."""

    selector: str
    sysroot: Path
    tools: Mapping[str, Path]


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


def _resolve_exact_rustc(
    rustc: Path,
    channel: str,
    repo: Path,
    environment: Mapping[str, str],
) -> PinnedRustToolchain:
    """Revalidate one compiler and its complete canonical sysroot closure."""

    version = _run_query((str(rustc), "-vV"), repo, environment, "toolchain_invalid")
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

    sysroot_output = _run_query(
        (str(rustc), "--print", "sysroot"),
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

    selected: dict[str, Path] = {}
    for name in _RUST_TOOLS:
        executable = _validated_executable(
            rust_bin / name, "toolchain_component_missing"
        )
        if executable.parent != rust_bin:
            raise ToolchainError("toolchain_invalid")
        selected[name] = executable
    return PinnedRustToolchain(f"{channel}-{host}", sysroot, selected)


def resolve_pinned_rust_tools(
    repo: Path, environment: Mapping[str, str]
) -> PinnedRustToolchain:
    """Return an exact selector and verified tools from its canonical sysroot.

    The pin must be a concrete version with the Clippy and rustfmt components.
    An inherited exact compiler capability is revalidated directly so nested
    sandboxes do not need rustup metadata. Otherwise every executable must
    resolve through ``rustup which`` to the same installed sysroot. Missing or
    ambiguous inputs fail closed with a stable structured-result reason.
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

    exact_selector = environment.get("RUSTUP_TOOLCHAIN")
    rustc_value = shutil.which("rustc", path=environment.get("PATH"))
    if exact_selector and rustc_value:
        direct_rustc = _validated_executable(
            Path(rustc_value), "toolchain_component_missing"
        )
        if direct_rustc.name == "rustc":
            direct = _resolve_exact_rustc(direct_rustc, channel, repo, environment)
            if direct.tools["rustc"] == direct_rustc:
                if direct.selector != exact_selector:
                    raise ToolchainError("toolchain_invalid")
                return direct

    rustup_value = shutil.which("rustup", path=environment.get("PATH"))
    if not rustup_value:
        raise ToolchainError("toolchain_component_missing")
    rustup = _validated_executable(Path(rustup_value), "toolchain_component_missing")
    selected_rustc = _rustup_which(rustup, channel, "rustc", repo, environment)
    resolved = _resolve_exact_rustc(selected_rustc, channel, repo, environment)
    for name in _RUST_TOOLS:
        executable = _rustup_which(rustup, resolved.selector, name, repo, environment)
        if executable != resolved.tools[name]:
            raise ToolchainError("toolchain_invalid")
    return resolved
