"""Linux capability boundary for reusable static quality-gate execution.

The coordinator prepares a tracked-source snapshot, then runs every collector and
gate process in a Bubblewrap PID/mount/network namespace.  The checkout's real
``.csa`` tree is never mounted into that namespace; only the target cache and a
private temporary directory are writable.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Mapping, Sequence

from quality_gate_environment import PRIVATE_BIN_PATH, normalized_static_environment
from quality_gate_process import ProcessResult, collect_bounded, execute_supervised

__all__ = (
    "GateSandbox",
    "IsolationError",
)

BWRAP = Path("/usr/bin/bwrap")
GIT = Path("/usr/bin/git")

_REQUIRED_TOOLS = ("bash", "cargo", "git", "python3", "rustc")
_OPTIONAL_TOOLS = (
    "ar",
    "cargo-deny",
    "cargo-nextest",
    "cargo-clippy",
    "cc",
    "clippy-driver",
    "just",
    "ld",
    "lefthook",
    "make",
    "pkg-config",
    "rg",
    "rustdoc",
    "rustfmt",
    "shellcheck",
    "timeout",
)
_FIXED_TOOLS = {
    "bash": Path("/usr/bin/bash"),
    "git": GIT,
    "python3": Path(sys.executable).resolve(),
    "rg": Path("/usr/bin/rg"),
}


class IsolationError(RuntimeError):
    """Required sandbox preparation or containment could not be proven."""


def _safe_git_environment() -> dict[str, str]:
    return {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_SYSTEM": "/dev/null",
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
    }


def _run_git(repo: Path, *arguments: str) -> bytes:
    try:
        completed = subprocess.run(
            (str(GIT), *arguments),
            cwd=repo,
            env=_safe_git_environment(),
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


def _host_clean_state(repo: Path) -> tuple[str, str, str]:
    statuses: list[str] = []
    for arguments in (
        ("diff", "--cached", "--quiet", "--ignore-submodules", "--"),
        ("diff", "--quiet", "--ignore-submodules", "--"),
    ):
        try:
            completed = subprocess.run(
                (str(GIT), *arguments),
                cwd=repo,
                env=_safe_git_environment(),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                check=False,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise IsolationError("source clean-state unavailable") from error
        if completed.returncode not in {0, 1}:
            raise IsolationError("source clean-state unavailable")
        statuses.append(str(completed.returncode == 0).lower())
    untracked = _run_git(repo, "ls-files", "--others", "--exclude-standard", "-z")
    return statuses[0], statuses[1], hashlib.sha256(untracked).hexdigest()


def _tracked_entries(repo: Path) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []
    for record in _run_git(repo, "ls-files", "--stage", "-z").split(b"\0"):
        if not record:
            continue
        try:
            metadata, raw_path = record.split(b"\t", 1)
            mode, _oid, stage = metadata.decode("ascii").split()
            path = os.fsdecode(raw_path)
        except (UnicodeError, ValueError) as error:
            raise IsolationError("source snapshot metadata invalid") from error
        if stage != "0" or mode not in {"100644", "100755", "120000"}:
            raise IsolationError("source snapshot contains unsupported entries")
        candidate = Path(path)
        if candidate.is_absolute() or ".." in candidate.parts:
            raise IsolationError("source snapshot path invalid")
        entries.append((mode, path))
    return entries


def _read_tracked_value(repo: Path, mode: str, relative: str) -> bytes:
    path = repo / relative
    try:
        if mode == "120000":
            return os.fsencode(os.readlink(path))
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as error:
        raise IsolationError("tracked source changed during snapshot") from error
    try:
        status = os.fstat(descriptor)
        if not stat.S_ISREG(status.st_mode):
            raise IsolationError("tracked source is not a regular file")
        digest_input = bytearray()
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest_input.extend(chunk)
        return bytes(digest_input)
    finally:
        os.close(descriptor)


def _source_fingerprint(
    repo: Path,
    entries: Sequence[tuple[str, str]],
    excluded_prefix: str | None = None,
) -> str:
    digest = hashlib.sha256()
    digest.update(_run_git(repo, "rev-parse", "HEAD").strip())
    digest.update(_run_git(repo, "ls-files", "--stage", "-z"))
    untracked = _run_git(repo, "ls-files", "--others", "--exclude-standard", "-z")
    if excluded_prefix is not None:
        prefix = os.fsencode(excluded_prefix)
        untracked = b"\0".join(
            path
            for path in untracked.split(b"\0")
            if path and path != prefix and not path.startswith(prefix + b"/")
        )
        if untracked:
            untracked += b"\0"
    digest.update(untracked)
    for mode, relative in entries:
        raw_path = os.fsencode(relative)
        value = _read_tracked_value(repo, mode, relative)
        digest.update(mode.encode("ascii"))
        digest.update(len(raw_path).to_bytes(8, "big"))
        digest.update(raw_path)
        digest.update(len(value).to_bytes(8, "big"))
        digest.update(value)
    return digest.hexdigest()


def _copy_snapshot(
    repo: Path, destination: Path, entries: Sequence[tuple[str, str]]
) -> None:
    destination.mkdir(mode=0o700)
    for mode, relative in entries:
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        value = _read_tracked_value(repo, mode, relative)
        if mode == "120000":
            link = os.fsdecode(value)
            resolved = (target.parent / link).resolve(strict=False)
            try:
                resolved.relative_to(destination)
            except ValueError:
                link = "/.quality-gate-external-symlink-unavailable"
            os.symlink(link, target)
            continue
        descriptor = os.open(
            target,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC,
            0o755 if mode == "100755" else 0o644,
        )
        try:
            offset = 0
            while offset < len(value):
                offset += os.write(descriptor, value[offset:])
        finally:
            os.close(descriptor)
    (destination / ".git").mkdir(exist_ok=True)
    (destination / ".csa").mkdir(mode=0o700, exist_ok=True)
    (destination / "target").mkdir(exist_ok=True)


def _project_rust_bin(repo: Path) -> Path | None:
    """Select the repository-pinned mise toolchain without invoking a shim."""

    toolchain_file = repo / "rust-toolchain.toml"
    try:
        configuration = tomllib.loads(toolchain_file.read_text(encoding="utf-8"))
        channel = configuration["toolchain"]["channel"]
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, KeyError, TypeError):
        return None
    if not isinstance(channel, str) or not channel or "/" in channel or ".." in channel:
        return None
    root = Path("/usr/local/share/mise/installs/rust/stable/toolchains")
    candidates = sorted(root.glob(f"{channel}-*"))
    exact = root / channel
    if exact.exists():
        candidates.append(exact)
    for candidate in candidates:
        binary = candidate / "bin"
        if (binary / "cargo").is_file() and (binary / "rustc").is_file():
            return binary.resolve(strict=True)
    return None


def _selected_tools(repo: Path, environment: Mapping[str, str]) -> dict[str, Path]:
    selected: dict[str, Path] = {}
    search_path = environment.get("PATH", os.defpath)
    for name in (*_REQUIRED_TOOLS, *_OPTIONAL_TOOLS):
        candidate = _FIXED_TOOLS.get(name)
        if candidate is None:
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
        if not stat.S_ISREG(status.st_mode) or not os.access(resolved, os.X_OK):
            raise IsolationError("sandbox tool provenance invalid")
        selected[name] = resolved
    rust_bin = _project_rust_bin(repo)
    if rust_bin is not None:
        for name in (
            "cargo",
            "cargo-clippy",
            "clippy-driver",
            "rustc",
            "rustdoc",
            "rustfmt",
        ):
            candidate = rust_bin / name
            current = selected.get(name)
            if not candidate.is_file() or not os.access(candidate, os.X_OK):
                continue
            if current is None or current.name in {"mise", "rustup"}:
                selected[name] = candidate.resolve(strict=True)
    return selected


def _visible_in_sandbox(repo: Path, path: Path) -> bool:
    """Return whether the prepared mount plan exposes this exact host path."""

    if path.is_relative_to(repo):
        return True
    return not any(
        path.is_relative_to(masked)
        for masked in map(Path, ("/home", "/root", "/run", "/tmp", "/var/tmp"))
    )


class GateSandbox:
    """Prepared static-gate snapshot and namespace execution plan."""

    def __init__(self, repo: Path, environment: Mapping[str, str]) -> None:
        self.repo = repo
        self._test_failure = environment.get("CSA_QUALITY_GATE_TEST_ISOLATION_FAILURE")
        self.entries = _tracked_entries(repo)
        self.clean_state = _host_clean_state(repo)
        self.source_fingerprint = _source_fingerprint(repo, self.entries)
        try:
            self.target = (repo / "target").resolve(strict=True)
        except OSError as error:
            raise IsolationError("sandbox target cache unavailable") from error
        sandbox_root = self.target / "quality-gate-sandboxes"
        self._created_sandbox_root = not sandbox_root.exists()
        sandbox_root.mkdir(mode=0o700, exist_ok=True)
        self._temporary_owner = tempfile.TemporaryDirectory(
            prefix="quality-gate.", dir=sandbox_root
        )
        self._temporary = Path(self._temporary_owner.name)
        self._excluded_host_prefix: str | None
        try:
            self._excluded_host_prefix = os.fspath(self._temporary.relative_to(repo))
        except ValueError:
            self._excluded_host_prefix = None
        self.snapshot = self._temporary / "source"
        self.private_tmp = self._temporary / "tmp"
        self.private_bin = self._temporary / "bin"
        self.empty_file = self._temporary / "empty"
        self.private_tmp.mkdir(mode=0o700)
        self.private_bin.mkdir(mode=0o700)
        self.empty_file.touch(mode=0o600)
        _copy_snapshot(repo, self.snapshot, self.entries)
        self.environment = normalized_static_environment(
            environment,
            self.source_fingerprint,
            self.clean_state,
        )
        self.tools = _selected_tools(repo, environment)
        self.tool_mounts: dict[str, Path] = {}
        self.explicit_tools: dict[str, Path] = {}
        self.data_mounts: dict[str, Path] = {}
        for name, executable in self.tools.items():
            destination = self.private_bin / name
            if _visible_in_sandbox(repo, executable):
                os.symlink(executable, destination)
            else:
                destination.touch(mode=0o700)
                self.tool_mounts[name] = executable
        for variable in ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            value = environment.get(variable)
            if not value:
                continue
            if not Path(value).is_absolute():
                self.close()
                raise IsolationError("explicit Rust tool provenance invalid")
            try:
                executable = Path(value).resolve(strict=True)
                status = executable.stat()
            except OSError as error:
                self.close()
                raise IsolationError("explicit Rust tool provenance invalid") from error
            if not stat.S_ISREG(status.st_mode) or not os.access(executable, os.X_OK):
                self.close()
                raise IsolationError("explicit Rust tool provenance invalid")
            mount_name = "explicit-" + variable.lower().replace("_", "-")
            destination = self.private_bin / mount_name
            if _visible_in_sandbox(repo, executable):
                os.symlink(executable, destination)
            else:
                destination.touch(mode=0o700)
                self.explicit_tools[mount_name] = executable
            self.environment[variable] = f"{PRIVATE_BIN_PATH}/{mount_name}"
        target_value = environment.get("CARGO_BUILD_TARGET")
        if target_value:
            target_path = Path(target_value)
            candidate = target_path if target_path.is_absolute() else repo / target_path
            if candidate.exists() or candidate.is_symlink():
                try:
                    resolved_target = candidate.resolve(strict=True)
                    target_status = resolved_target.stat()
                except OSError as error:
                    self.close()
                    raise IsolationError("Cargo target provenance invalid") from error
                if not stat.S_ISREG(target_status.st_mode):
                    self.close()
                    raise IsolationError("Cargo target provenance invalid")
                mount_name = "cargo-target-spec.json"
                (self.private_bin / mount_name).touch(mode=0o600)
                self.data_mounts[mount_name] = resolved_target
                self.environment["CARGO_BUILD_TARGET"] = (
                    f"{PRIVATE_BIN_PATH}/{mount_name}"
                )
        try:
            git_dir_raw = _run_git(repo, "rev-parse", "--absolute-git-dir").strip()
            self.git_dir = Path(os.fsdecode(git_dir_raw)).resolve(strict=True)
        except OSError as error:
            self.close()
            raise IsolationError("sandbox mount source unavailable") from error
        if not self.target.is_dir() or not self.git_dir.is_dir():
            self.close()
            raise IsolationError("sandbox mount source invalid")

    def close(self) -> None:
        owner = getattr(self, "_temporary_owner", None)
        if owner is not None:
            owner.cleanup()
            del self._temporary_owner
            del self._temporary
        if getattr(self, "_created_sandbox_root", False):
            try:
                (self.target / "quality-gate-sandboxes").rmdir()
            except OSError:
                pass

    def __enter__(self) -> "GateSandbox":
        return self

    def __exit__(self, _kind: object, _value: object, _traceback: object) -> None:
        self.close()

    def current_source_fingerprint(self) -> str:
        """Re-read the host tracked source after the sandbox tree is gone."""

        return _source_fingerprint(
            self.repo,
            _tracked_entries(self.repo),
            self._excluded_host_prefix,
        )

    def _cargo_home(self) -> Path:
        value = self.environment.get("CARGO_HOME")
        if not value or value == "/usr/local":
            return Path("/usr/local/share/cargo")
        return Path(value)

    def _sandbox_arguments(self) -> list[str]:
        if not BWRAP.is_file() or not os.access(BWRAP, os.X_OK):
            raise IsolationError("required isolation unavailable")
        args = [
            str(BWRAP),
            "--ro-bind",
            "/",
            "/",
            "--tmpfs",
            "/home",
            "--tmpfs",
            "/root",
            "--tmpfs",
            "/run",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/var/tmp",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
        ]
        destination = self.repo
        current = Path("/")
        for component in destination.parts[1:-1]:
            current /= component
            args.extend(("--dir", str(current)))
        args.extend(("--ro-bind", str(self.snapshot), str(destination)))
        args.extend(("--ro-bind", str(self.git_dir), str(destination / ".git")))
        args.extend(("--bind", str(self.target), str(destination / "target")))
        args.extend(("--tmpfs", str(destination / ".csa")))
        args.extend(("--dir", PRIVATE_BIN_PATH))
        args.extend(("--ro-bind", str(self.private_bin), PRIVATE_BIN_PATH))
        for name, executable in sorted(self.tool_mounts.items()):
            args.extend(("--ro-bind", str(executable), f"{PRIVATE_BIN_PATH}/{name}"))
        for name, executable in sorted(self.explicit_tools.items()):
            args.extend(("--ro-bind", str(executable), f"{PRIVATE_BIN_PATH}/{name}"))
        for name, source in sorted(self.data_mounts.items()):
            args.extend(("--ro-bind", str(source), f"{PRIVATE_BIN_PATH}/{name}"))
        args.extend(("--dir", "/run/csa-mise-disabled"))

        cargo_home = self._cargo_home()
        if not cargo_home.is_absolute():
            raise IsolationError("Cargo home is not absolute")
        args.extend(("--tmpfs", str(cargo_home)))
        host_cargo_home = cargo_home
        if host_cargo_home.is_dir():
            for child in ("bin", "git", "registry"):
                source = host_cargo_home / child
                if source.exists():
                    args.extend(
                        ("--ro-bind", str(source.resolve()), str(cargo_home / child))
                    )
        for name in ("config", "config.toml", "credentials", "credentials.toml"):
            args.extend(("--ro-bind", str(self.empty_file), str(cargo_home / name)))

        tmpdir = Path(self.environment["TMPDIR"])
        if not tmpdir.is_absolute():
            raise IsolationError("temporary directory is not absolute")
        if tmpdir != Path("/tmp"):
            args.extend(("--bind", str(self.private_tmp), str(tmpdir)))

        args.extend(
            (
                "--unshare-user",
                "--unshare-ipc",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-uts",
                "--unshare-cgroup-try",
                "--disable-userns",
                "--as-pid-1",
                "--new-session",
                "--die-with-parent",
                "--clearenv",
            )
        )
        for name, value in sorted(self.environment.items()):
            args.extend(("--setenv", name, value))
        args.extend(("--chdir", str(self.repo)))
        return args

    def _command(self, command: Sequence[str], *, normalize: bool) -> list[str]:
        args = self._sandbox_arguments()
        args.append("--")
        args.extend(("/usr/bin/bash", "-c", 'umask 022; exec "$@"', "csa-static"))
        if normalize:
            args.append("scripts/cargo-env-normalize.sh")
        args.extend(command)
        return args

    def preflight(self) -> None:
        """Prove namespace, state masking, target, tmp, and descriptor isolation."""

        if self._test_failure == "missing":
            raise IsolationError("required isolation unavailable")

        parent_mount = os.readlink("/proc/self/ns/mnt")
        parent_net = os.readlink("/proc/self/ns/net")
        script = r"""
set -euo pipefail
test ! -e .csa/state
printf sandbox >.csa/preflight
probe="target/.quality-gate-sandbox-probe.$$"
printf target >"$probe"
rm -f "$probe"
mkdir -p "${TMPDIR}/quality-gate-preflight"
rmdir "${TMPDIR}/quality-gate-preflight"
printf cache >"$1/.quality-gate-cache-probe"
rm -f "$1/.quality-gate-cache-probe"
for descriptor in /proc/[0-9]*/fd/*; do
  target="$(readlink "$descriptor" 2>/dev/null || true)"
  case "$target" in *quality-gate-receipts*) exit 91 ;; esac
done
printf '%s\n%s\n' "$(readlink /proc/self/ns/mnt)" "$(readlink /proc/self/ns/net)"
"""
        try:
            completed = subprocess.run(
                self._command(
                    (
                        "/usr/bin/bash",
                        "-c",
                        script,
                        "quality-gate-preflight",
                        str(self._cargo_home()),
                    ),
                    normalize=False,
                ),
                cwd=self.repo,
                env={},
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=20,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise IsolationError("required isolation unavailable") from error
        lines = completed.stdout.decode("ascii", errors="replace").splitlines()
        if (
            completed.returncode != 0
            or len(lines) != 2
            or lines[0] == parent_mount
            or lines[1] == parent_net
            or (self.repo / ".csa/preflight").exists()
        ):
            raise IsolationError("required isolation unavailable")

    def collect(self, command: Sequence[str]) -> bytes:
        """Collect canonical provenance inside the exact static-gate sandbox."""

        collector = (
            str(Path(sys.executable).resolve()),
            "scripts/quality-gate-state.py",
            "collect",
            "--repo",
            str(self.repo),
            "--",
            *command,
        )
        try:
            return collect_bounded(self._command(collector, normalize=True), self.repo)
        except RuntimeError as error:
            raise IsolationError("sandboxed provenance collection failed") from error

    def execute(self, command: Sequence[str]) -> ProcessResult:
        """Run the static gate and synchronously terminate its PID namespace on signal."""

        return execute_supervised(
            self._command(command, normalize=True),
            self.repo,
            inject_start_failure=self._test_failure == "start",
        )
