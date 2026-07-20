"""Resolve the repository-pinned Rust toolchain without ambient proxies.

The quality-gate sandbox consumes only verified executable realpaths from one
complete, already-installed toolchain.  Resolution happens before namespace
entry so the sandbox never needs rustup metadata or network capability.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Mapping, Sequence

__all__ = (
    "PinnedRustToolchain",
    "SANDBOX_RUST_TOOLCHAIN_ROOT",
    "ToolchainError",
    "resolve_pinned_rust_tools",
)

SANDBOX_RUST_TOOLCHAIN_ROOT = Path("/run/csa-rust-toolchain")
MAX_LAUNCHER_BYTES = 256 * 1024 * 1024

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
class ValidatedLauncher:
    """An admitted argv[0] paired with a validated canonical terminal file."""

    invocation: Path
    terminal: Path
    fingerprint: tuple[int, int, int, int, int, int, int]
    terminal_sha256: str
    semantics_sha256: str
    authority_sha256: str

    def run(
        self,
        arguments: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
        timeout: int,
        reason: str,
    ) -> subprocess.CompletedProcess[bytes]:
        """Execute the opened terminal while retaining the admitted argv[0]."""

        if any("\0" in argument for argument in arguments):
            raise ToolchainError(reason)
        descriptor = _open_terminal(self.terminal, reason)
        try:
            if _file_fingerprint(os.fstat(descriptor)) != self.fingerprint:
                raise ToolchainError(reason)
            if self.invocation.name == self.terminal.name:
                return subprocess.run(
                    (os.fspath(self.invocation), *arguments),
                    executable=f"/proc/self/fd/{descriptor}",
                    pass_fds=(descriptor,),
                    cwd=cwd,
                    env=dict(env),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                    timeout=timeout,
                )
            with tempfile.TemporaryDirectory(prefix="quality-gate-launcher.") as root:
                executable = Path(root) / self.invocation.name
                _copy_sealed_terminal(
                    descriptor, executable, self.terminal_sha256, reason
                )
                return subprocess.run(
                    (os.fspath(self.invocation), *arguments),
                    executable=os.fspath(executable),
                    cwd=cwd,
                    env=dict(env),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                    timeout=timeout,
                )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ToolchainError(reason) from error
        finally:
            os.close(descriptor)


@dataclass(frozen=True)
class PinnedRustToolchain:
    """Canonical installed sysroot and every executable required by the gate."""

    selector: str
    sysroot: Path
    tools: Mapping[str, Path]
    launcher_invocation_sha256: str
    launcher_authority_sha256: str
    semantic_projection: str


def _file_fingerprint(
    status: os.stat_result,
) -> tuple[int, int, int, int, int, int, int]:
    return (
        status.st_dev,
        status.st_ino,
        status.st_uid,
        stat.S_IMODE(status.st_mode),
        status.st_size,
        status.st_mtime_ns,
        status.st_ctime_ns,
    )


def _validate_file_status(status: os.stat_result, reason: str) -> None:
    if (
        not stat.S_ISREG(status.st_mode)
        or not status.st_mode & 0o111
        or stat.S_IMODE(status.st_mode) & stat.S_IWOTH
        or status.st_uid not in {0, os.geteuid()}
        or status.st_size > MAX_LAUNCHER_BYTES
    ):
        raise ToolchainError(reason)


def _open_terminal(path: Path, reason: str) -> int:
    try:
        descriptor = os.open(
            path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
        )
    except OSError as error:
        raise ToolchainError(reason) from error
    try:
        _validate_file_status(os.fstat(descriptor), reason)
    except ToolchainError:
        os.close(descriptor)
        raise
    return descriptor


def _hash_descriptor(descriptor: int, status: os.stat_result, reason: str) -> str:
    digest = hashlib.sha256()
    offset = 0
    while offset < status.st_size:
        try:
            chunk = os.pread(
                descriptor, min(1024 * 1024, status.st_size - offset), offset
            )
        except OSError as error:
            raise ToolchainError(reason) from error
        if not chunk:
            raise ToolchainError(reason)
        digest.update(chunk)
        offset += len(chunk)
    try:
        if os.pread(descriptor, 1, status.st_size):
            raise ToolchainError(reason)
    except OSError as error:
        raise ToolchainError(reason) from error
    if _file_fingerprint(os.fstat(descriptor)) != _file_fingerprint(status):
        raise ToolchainError(reason)
    return digest.hexdigest()


def _copy_sealed_terminal(
    source: int, destination: Path, expected_sha256: str, reason: str
) -> None:
    """Materialize one private read-only launcher image with its admitted basename."""

    try:
        descriptor = os.open(
            destination,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
            0o500,
        )
    except OSError as error:
        raise ToolchainError(reason) from error
    digest = hashlib.sha256()
    offset = 0
    try:
        size = os.fstat(source).st_size
        while offset < size:
            chunk = os.pread(source, min(1024 * 1024, size - offset), offset)
            if not chunk:
                raise ToolchainError(reason)
            digest.update(chunk)
            written = 0
            while written < len(chunk):
                count = os.write(descriptor, chunk[written:])
                if count <= 0:
                    raise ToolchainError(reason)
                written += count
            offset += len(chunk)
        if digest.hexdigest() != expected_sha256:
            raise ToolchainError(reason)
        os.fsync(descriptor)
    except OSError as error:
        raise ToolchainError(reason) from error
    finally:
        os.close(descriptor)
    try:
        os.chmod(destination, 0o500)
    except OSError as error:
        raise ToolchainError(reason) from error


def _digest_fields(fields: Mapping[str, str]) -> str:
    digest = hashlib.sha256()
    for name in sorted(fields):
        encoded_name = os.fsencode(name)
        encoded_value = os.fsencode(fields[name])
        digest.update(len(encoded_name).to_bytes(8, "big"))
        digest.update(encoded_name)
        digest.update(len(encoded_value).to_bytes(8, "big"))
        digest.update(encoded_value)
    return digest.hexdigest()


def _query_semantics(arguments: Sequence[str]) -> str:
    digest = hashlib.sha256()
    for argument in arguments:
        encoded = os.fsencode(argument)
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def _invocation_provenance(
    launchers: Mapping[str, ValidatedLauncher],
    queries: Mapping[str, Sequence[Sequence[str]]],
    selector: str | None,
) -> str:
    fields = {"selector": selector if selector is not None else "unset"}
    for name, launcher in sorted(launchers.items()):
        fields[f"{name}:launcher"] = launcher.semantics_sha256
        for index, arguments in enumerate(queries.get(name, ())):
            fields[f"{name}:query:{index}"] = _query_semantics(arguments)
    return _digest_fields(fields)


def _launcher_symlink_semantics(candidate: Path, reason: str) -> tuple[Path, str]:
    if not candidate.is_absolute():
        raise ToolchainError(reason)
    current = Path(os.path.abspath(os.fspath(candidate)))
    seen: set[Path] = set()
    fields: dict[str, str] = {}
    for depth in range(41):
        if current in seen:
            raise ToolchainError(reason)
        seen.add(current)
        try:
            status = current.lstat()
        except OSError as error:
            raise ToolchainError(reason) from error
        metadata = _file_fingerprint(status)
        fields[f"{depth}:path"] = os.fspath(current)
        fields[f"{depth}:metadata"] = ":".join(map(str, metadata))
        if not stat.S_ISLNK(status.st_mode):
            try:
                return current.resolve(strict=True), _digest_fields(fields)
            except OSError as error:
                raise ToolchainError(reason) from error
        try:
            target = os.readlink(current)
        except OSError as error:
            raise ToolchainError(reason) from error
        fields[f"{depth}:target"] = target
        next_path = Path(target) if os.path.isabs(target) else current.parent / target
        current = Path(os.path.abspath(os.path.normpath(os.fspath(next_path))))
    raise ToolchainError(reason)


def _validated_launcher(candidate: Path, reason: str) -> ValidatedLauncher:
    terminal, semantics_sha256 = _launcher_symlink_semantics(candidate, reason)
    descriptor = _open_terminal(terminal, reason)
    try:
        status = os.fstat(descriptor)
        terminal_sha256 = _hash_descriptor(descriptor, status, reason)
    finally:
        os.close(descriptor)
    authority_sha256 = _digest_fields(
        {
            "terminal": os.fspath(terminal),
            "terminal_metadata": ":".join(map(str, _file_fingerprint(status))),
            "terminal_sha256": terminal_sha256,
        }
    )
    return ValidatedLauncher(
        invocation=candidate,
        terminal=terminal,
        fingerprint=_file_fingerprint(status),
        terminal_sha256=terminal_sha256,
        semantics_sha256=semantics_sha256,
        authority_sha256=authority_sha256,
    )


def _validated_executable(candidate: Path, reason: str) -> Path:
    try:
        resolved = candidate.resolve(strict=True)
        status = resolved.stat()
    except OSError as error:
        raise ToolchainError(reason) from error
    _validate_file_status(status, reason)
    return resolved


def _run_launcher_query(
    launcher: ValidatedLauncher,
    arguments: Sequence[str],
    repo: Path,
    environment: Mapping[str, str],
    reason: str,
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
    selector = environment.get("RUSTUP_TOOLCHAIN")
    if selector:
        query_environment["RUSTUP_TOOLCHAIN"] = selector
    try:
        completed = launcher.run(
            arguments,
            cwd=repo,
            env=query_environment,
            timeout=15,
            reason=reason,
        )
    except ToolchainError:
        raise
    except OSError as error:
        raise ToolchainError(reason) from error
    if completed.returncode != 0 or len(completed.stdout) > 4096:
        raise ToolchainError(reason)
    return completed.stdout


def _rustup_which(
    rustup: ValidatedLauncher,
    selector: str,
    tool: str,
    repo: Path,
    environment: Mapping[str, str],
) -> Path:
    output = _run_launcher_query(
        rustup,
        ("which", "--toolchain", selector, tool),
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
    rustc: ValidatedLauncher,
    channel: str,
    repo: Path,
    environment: Mapping[str, str],
    launchers: Mapping[str, ValidatedLauncher],
    queries: Mapping[str, Sequence[Sequence[str]]],
    selector_class: str,
) -> PinnedRustToolchain:
    """Revalidate one compiler and its complete canonical sysroot closure."""

    version = _run_launcher_query(
        rustc, ("-vV",), repo, environment, "toolchain_invalid"
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

    sysroot_output = _run_launcher_query(
        rustc,
        ("--print", "sysroot"),
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
    primary = launchers.get("rustup", rustc)
    authority_fields = {
        name: launcher.authority_sha256 for name, launcher in sorted(launchers.items())
    }
    projection = ";".join(
        (
            f"selector-{selector_class}",
            f"invocation-{primary.invocation.name}",
            f"terminal-{primary.terminal.name}",
            "terminal-digest-sha256",
            "sysroot-verified-host-to-static",
            "mount-depth-1",
            "umask-022",
            f"query-launchers-{len(launchers)}",
        )
    )
    return PinnedRustToolchain(
        f"{channel}-{host}",
        sysroot,
        selected,
        _invocation_provenance(launchers, queries, environment.get("RUSTUP_TOOLCHAIN")),
        _digest_fields(authority_fields),
        projection,
    )


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
        direct_rustc = _validated_launcher(
            Path(rustc_value), "toolchain_component_missing"
        )
        if direct_rustc.invocation.name == "rustc":
            direct = _resolve_exact_rustc(
                direct_rustc,
                channel,
                repo,
                environment,
                {"rustc": direct_rustc},
                {"rustc": (("-vV",), ("--print", "sysroot"))},
                "exact",
            )
            if direct.selector != exact_selector:
                raise ToolchainError("toolchain_invalid")
            return direct

    rustup_value = shutil.which("rustup", path=environment.get("PATH"))
    if not rustup_value:
        raise ToolchainError("toolchain_component_missing")
    rustup = _validated_launcher(Path(rustup_value), "toolchain_component_missing")
    selected_rustc = _rustup_which(rustup, channel, "rustc", repo, environment)
    rustc = _validated_launcher(selected_rustc, "toolchain_component_missing")
    resolved = _resolve_exact_rustc(
        rustc,
        channel,
        repo,
        environment,
        {"rustup": rustup, "rustc": rustc},
        {
            "rustup": (("which", "--toolchain", channel, "rustc"),),
            "rustc": (("-vV",), ("--print", "sysroot")),
        },
        "exact" if exact_selector else "unset",
    )
    rustup_queries: list[tuple[str, ...]] = [("which", "--toolchain", channel, "rustc")]
    for name in _RUST_TOOLS:
        executable = _rustup_which(rustup, resolved.selector, name, repo, environment)
        rustup_queries.append(("which", "--toolchain", resolved.selector, name))
        if executable != resolved.tools[name]:
            raise ToolchainError("toolchain_invalid")
    return replace(
        resolved,
        launcher_invocation_sha256=_invocation_provenance(
            {"rustup": rustup, "rustc": rustc},
            {
                "rustup": rustup_queries,
                "rustc": (("-vV",), ("--print", "sysroot")),
            },
            exact_selector,
        ),
    )
