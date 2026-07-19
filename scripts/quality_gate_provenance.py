#!/usr/bin/env python3
"""Secure exact-input quality-gate receipt provenance collection."""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
from pathlib import Path
from typing import Sequence, TypeGuard

from quality_gate_secure_state import (
    IMPLEMENTATION_VERSION,
    SCHEMA_VERSION,
    sha256_bytes,
)

__all__ = (
    "MAX_MANIFEST_BYTES",
    "ProvenanceError",
    "collect_manifest",
)

MAX_MANIFEST_BYTES = 256 * 1024
MAX_REPOSITORY_FILE_BYTES = 32 * 1024 * 1024
MAX_TOOL_BYTES = 256 * 1024 * 1024
MAX_TOOLCHAIN_ENTRIES = 4096
TOOLCHAIN_CONTENT_HASH_LIMIT = 1024 * 1024
COMMAND_TIMEOUT_SECONDS = 15

ENV_PREFIXES = ("CARGO_", "RUST", "NEXTEST_", "MISE_")
ENV_EXACT = {
    "AR",
    "BINDGEN_EXTRA_CLANG_ARGS",
    "CC",
    "CFLAGS",
    "CPP",
    "CPPFLAGS",
    "CSA_PRESERVE_CARGO_TARGET_DIR",
    "CXX",
    "CXXFLAGS",
    "LD",
    "LDFLAGS",
    "PKG_CONFIG_PATH",
    "CSA_QUALITY_GATE_SANDBOX_VERSION",
    "CSA_QUALITY_GATE_SOURCE_SNAPSHOT_SHA256",
    "CSA_QUALITY_GATE_TOOLCHAIN_AUTHORITY_SHA256",
    "CSA_QUALITY_GATE_TOOLCHAIN_INVOCATION_SHA256",
    "CSA_QUALITY_GATE_TOOLCHAIN_SEMANTIC_PROJECTION",
    # Ambient identity/path vars pinned by the sanitizer.
    "HOME",
    "LOGNAME",
    "SHELL",
    "TERM",
    "TMPDIR",
    "USER",
}
ENV_NORMALIZED_SEPARATELY = {
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
}
ENV_VOLATILE = {
    "CARGO_MAKEFLAGS",
    "CARGO_TARGET_TMPDIR",
    "RUST_RECURSION_COUNT",
}
SECRET_MARKERS = ("AUTH", "CREDENTIAL", "KEY", "PASSWORD", "SECRET", "TOKEN")
PROVENANCE_TOOLS = (
    "ar",
    "bash",
    "cargo",
    "cargo-clippy",
    "cargo-fmt",
    "cargo-nextest",
    "cc",
    "clippy-driver",
    "git",
    "just",
    "ld",
    "lefthook",
    "make",
    "pkg-config",
    "python3",
    "rg",
    "rustc",
    "rustdoc",
    "rustfmt",
    "shellcheck",
    "timeout",
)


class ProvenanceError(RuntimeError):
    """An acceptance input could not be normalized deterministically."""


def encode_fields(fields: dict[str, str]) -> bytes:
    """Encode a canonical line-oriented manifest."""

    return "".join(f"{key}={fields[key]}\n" for key in sorted(fields)).encode()


def is_lower_sha256(value: object) -> TypeGuard[str]:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def run_checked(
    command: Sequence[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: int = COMMAND_TIMEOUT_SECONDS,
) -> bytes:
    """Run a bounded provenance command and return bounded stdout."""

    label = Path(command[0]).name
    if label == "git" and len(command) > 1:
        label = f"git:{command[1]}"

    try:
        completed = subprocess.run(
            list(command),
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ProvenanceError(f"provenance command unavailable: {label}") from error
    if completed.returncode != 0:
        raise ProvenanceError(f"provenance command failed: {label}")
    if len(completed.stdout) > MAX_MANIFEST_BYTES:
        raise ProvenanceError(f"provenance command output is too large: {label}")
    return completed.stdout


def git_output(repo: Path, *arguments: str) -> str:
    output = run_checked(("git", *arguments), cwd=repo)
    try:
        return output.decode("utf-8").strip()
    except UnicodeDecodeError as error:
        raise ProvenanceError("git provenance was not UTF-8") from error


def git_diff_is_clean(repo: Path, env: dict[str, str], *arguments: str) -> bool:
    return (
        subprocess.run(
            ("git", "diff", *arguments, "--quiet", "--ignore-submodules", "--"),
            cwd=repo,
            env=env,
            check=False,
        ).returncode
        == 0
    )


def hash_open_file(path: Path, maximum: int, *, resolve: bool = False) -> str:
    """Hash a bounded regular file without following its final component."""

    candidate = path.resolve(strict=True) if resolve else path
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    try:
        descriptor = os.open(candidate, flags)
    except OSError as error:
        raise ProvenanceError("required provenance file is unavailable") from error
    try:
        status = os.fstat(descriptor)
        if not stat.S_ISREG(status.st_mode) or status.st_size > maximum:
            raise ProvenanceError("provenance file is not a bounded regular file")
        digest = hashlib.sha256()
        remaining = status.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise ProvenanceError("provenance file was truncated while reading")
            digest.update(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise ProvenanceError("provenance file grew while reading")
        return digest.hexdigest()
    finally:
        os.close(descriptor)


def optional_repository_digest(repo: Path, relative: str) -> str:
    path = repo / relative
    try:
        return hash_open_file(path, MAX_REPOSITORY_FILE_BYTES)
    except (FileNotFoundError, ProvenanceError):
        if path.exists() or path.is_symlink():
            raise
        return "missing"


def command_digest(command: Sequence[str]) -> str:
    """Hash command arguments without shell quoting ambiguity."""

    encoded = bytearray()
    for argument in command:
        value = os.fsencode(argument)
        encoded.extend(len(value).to_bytes(8, "big"))
        encoded.extend(value)
    return sha256_bytes(bytes(encoded))


def resolve_executable(value: str, *, require_absolute: bool) -> Path:
    """Resolve an executable and reject ambiguous explicit paths."""

    if require_absolute and not os.path.isabs(value):
        raise ProvenanceError("explicit tool provenance must be absolute")
    located = value if os.path.isabs(value) else shutil.which(value)
    if not located:
        raise ProvenanceError("required provenance executable is missing")
    path = Path(located).resolve(strict=True)
    status = path.stat()
    if not stat.S_ISREG(status.st_mode) or not os.access(path, os.X_OK):
        raise ProvenanceError("provenance executable is not an executable regular file")
    return path


def toolchain_closure_provenance(sysroot: Path) -> str:
    """Bind compiler runtime/component closure: inode ctime+size for large libs, content hash for manifests."""

    digest = hashlib.sha256()
    digest.update(sha256_bytes(os.fsencode(sysroot)).encode())
    entries = 0
    for root_name in ("bin", "lib"):
        root = sysroot / root_name
        if not root.is_dir():
            raise ProvenanceError("Rust sysroot closure is incomplete")
        for directory, names, files in os.walk(root, followlinks=False):
            names.sort(key=os.fsencode)
            files.sort(key=os.fsencode)
            for name in (*names, *files):
                path = Path(directory) / name
                relative = path.relative_to(sysroot)
                try:
                    status = path.lstat()
                except OSError as error:
                    raise ProvenanceError("Rust sysroot closure changed") from error
                entries += 1
                if entries > MAX_TOOLCHAIN_ENTRIES:
                    raise ProvenanceError("Rust sysroot closure is too large")
                metadata = (
                    f"{relative}\0{status.st_mode:o}\0{status.st_uid}\0"
                    f"{status.st_gid}\0{status.st_dev}\0{status.st_ino}\0"
                    f"{status.st_size}\0{status.st_mtime_ns}\0{status.st_ctime_ns}\0"
                ).encode()
                digest.update(metadata)
                if stat.S_ISLNK(status.st_mode):
                    try:
                        digest.update(os.fsencode(os.readlink(path)))
                    except OSError as error:
                        raise ProvenanceError("Rust sysroot link changed") from error
                elif stat.S_ISREG(status.st_mode) and (
                    status.st_size <= TOOLCHAIN_CONTENT_HASH_LIMIT
                ):
                    digest.update(
                        hash_open_file(path, TOOLCHAIN_CONTENT_HASH_LIMIT).encode()
                    )
                elif not (stat.S_ISREG(status.st_mode) or stat.S_ISDIR(status.st_mode)):
                    raise ProvenanceError(
                        "Rust sysroot closure has unsupported entries"
                    )
    digest.update(str(entries).encode())
    return digest.hexdigest()


def compiler_provenance(repo: Path, env: dict[str, str]) -> tuple[str, str]:
    """Identify the normalized compiler, target, launchers, and their bytes."""

    explicit_rustc = env.get("RUSTC")
    selected_value = explicit_rustc or shutil.which("rustc", path=env.get("PATH"))
    if not selected_value:
        raise ProvenanceError("normalized rustc executable is missing")
    if explicit_rustc and not os.path.isabs(selected_value):
        raise ProvenanceError("explicit rustc provenance must be absolute")
    selected_launcher = Path(selected_value)
    selected = resolve_executable(selected_value, require_absolute=bool(explicit_rustc))
    sysroot_raw = run_checked(
        (str(selected_launcher), "--print", "sysroot"), cwd=repo, env=env
    )
    try:
        sysroot_text = sysroot_raw.decode("utf-8").strip()
    except UnicodeDecodeError as error:
        raise ProvenanceError("rustc sysroot was not UTF-8") from error
    if not os.path.isabs(sysroot_text):
        raise ProvenanceError("rustc sysroot provenance was not absolute")
    sysroot = Path(sysroot_text).resolve(strict=True)
    compiler = (sysroot / "bin" / "rustc").resolve(strict=True)
    if compiler.parent != (sysroot / "bin").resolve(strict=True):
        raise ProvenanceError("rustc escaped its canonical sysroot bin directory")
    version = run_checked((str(compiler), "-vV"), cwd=repo, env=env)
    host_lines = [
        line.split(b":", 1)[1].strip()
        for line in version.splitlines()
        if line.startswith(b"host:")
    ]
    if len(host_lines) != 1 or not host_lines[0]:
        raise ProvenanceError("rustc -vV did not contain one host")

    parts = {
        "compiler_bytes": hash_open_file(compiler, MAX_TOOL_BYTES),
        "compiler_version": sha256_bytes(version),
        "explicit_rustc": "unset",
        "rustc_wrapper": "unset",
        "rustc_workspace_wrapper": "unset",
        "sysroot_closure": toolchain_closure_provenance(sysroot),
    }
    if explicit_rustc:
        parts["explicit_rustc"] = hash_open_file(selected, MAX_TOOL_BYTES)
    for name, key in (
        ("RUSTC_WRAPPER", "rustc_wrapper"),
        ("RUSTC_WORKSPACE_WRAPPER", "rustc_workspace_wrapper"),
    ):
        value = env.get(name)
        if value:
            wrapper = resolve_executable(value, require_absolute=True)
            parts[key] = hash_open_file(wrapper, MAX_TOOL_BYTES)

    target = env.get("CARGO_BUILD_TARGET")
    if target:
        target_path = Path(target)
        candidate = target_path if target_path.is_absolute() else repo / target_path
        if candidate.exists() or candidate.is_symlink():
            target_value = "file:" + hash_open_file(
                candidate, MAX_REPOSITORY_FILE_BYTES
            )
        else:
            target_value = "triple:" + sha256_bytes(target.encode())
    else:
        target_value = "host:" + sha256_bytes(host_lines[0])
    return sha256_bytes(encode_fields(parts)), sha256_bytes(target_value.encode())


def toolchain_launcher_provenance(env: dict[str, str]) -> tuple[str, str, str]:
    """Validate the outer launcher identities injected by the static sandbox."""

    invocation = env.get("CSA_QUALITY_GATE_TOOLCHAIN_INVOCATION_SHA256")
    authority = env.get("CSA_QUALITY_GATE_TOOLCHAIN_AUTHORITY_SHA256")
    projection = env.get("CSA_QUALITY_GATE_TOOLCHAIN_SEMANTIC_PROJECTION")
    if (
        not is_lower_sha256(invocation)
        or not is_lower_sha256(authority)
        or not isinstance(projection, str)
        or not 0 < len(projection) <= 256
        or not all(
            character in "abcdefghijklmnopqrstuvwxyz0123456789-;"
            for character in projection
        )
    ):
        raise ProvenanceError("outer toolchain launcher provenance is unavailable")
    return invocation, authority, projection


def environment_provenance(env: dict[str, str]) -> str:
    """Hash every normalized acceptance-affecting Rust/Cargo/nextest input."""

    fields: dict[str, str] = {}
    for name in sorted(env):
        if name in ENV_VOLATILE or name in ENV_NORMALIZED_SEPARATELY:
            continue
        if name not in ENV_EXACT and not name.startswith(ENV_PREFIXES):
            continue
        value = env[name]
        if any(marker in name.upper() for marker in SECRET_MARKERS):
            fields[name] = "set" if value else "empty"
        else:
            fields[name] = sha256_bytes(os.fsencode(value))
    for required in (
        "CARGO_ENCODED_RUSTFLAGS",
        "MISE_DATA_DIR",
        "NEXTEST_PROFILE",
        "NEXTEST_TEST_THREADS",
    ):
        fields.setdefault(required, "unset")
    return sha256_bytes(encode_fields(fields))


def dotenv_provenance(repo: Path) -> str:
    """Hash ignored dotenv inputs without exposing their names or contents."""

    digest = hashlib.sha256()
    try:
        entries = sorted(repo.iterdir(), key=lambda path: os.fsencode(path.name))
    except OSError as error:
        raise ProvenanceError("could not enumerate dotenv provenance") from error
    for path in entries:
        if not path.name.startswith(".env"):
            continue
        if path.is_symlink():
            raise ProvenanceError("dotenv provenance cannot be a symlink")
        if not path.is_file():
            continue
        name_digest = sha256_bytes(os.fsencode(path.name))
        content_digest = hash_open_file(path, 1024 * 1024)
        digest.update(f"{name_digest}={content_digest}\n".encode())
    return digest.hexdigest()


def cargo_config_provenance(repo: Path, env: dict[str, str]) -> str:
    """Hash effective Cargo configuration files while excluding credentials."""

    fields: dict[str, str] = {}
    candidates = [repo / ".cargo" / "config", repo / ".cargo" / "config.toml"]
    cargo_home = env.get("CARGO_HOME")
    if cargo_home:
        if not os.path.isabs(cargo_home):
            raise ProvenanceError("normalized CARGO_HOME was not absolute")
        home = Path(cargo_home)
        candidates.extend((home / "config", home / "config.toml"))
    for index, path in enumerate(candidates):
        if path.is_symlink():
            raise ProvenanceError("Cargo config provenance cannot be a symlink")
        if path.exists():
            fields[str(index)] = hash_open_file(path, 4 * 1024 * 1024)
        else:
            fields[str(index)] = "missing"
    return sha256_bytes(encode_fields(fields))


def tool_provenance(repo: Path, env: dict[str, str]) -> str:
    """Hash executable bytes, cargo version, and explicit native tool overrides (CC/CXX/AR/LD/CPP)."""

    fields: dict[str, str] = {}
    for tool in PROVENANCE_TOOLS:
        located = shutil.which(tool, path=env.get("PATH"))
        if not located:
            fields[tool] = "missing"
            continue
        path = resolve_executable(located, require_absolute=True)
        fields[tool] = hash_open_file(path, MAX_TOOL_BYTES)
    cargo = shutil.which("cargo", path=env.get("PATH"))
    if not cargo:
        raise ProvenanceError("normalized Cargo executable is missing")
    fields["cargo_version"] = sha256_bytes(
        run_checked((cargo, "-vV"), cwd=repo, env=env)
    )
    for override in ("CC", "CXX", "AR", "LD", "CPP"):
        key = f"explicit-{override.lower()}"
        value = env.get(override)
        if not value:
            fields[key] = "unset"
            continue
        candidate = Path(value)
        try:
            resolved = candidate.resolve(strict=True)
            status = resolved.stat()
        except OSError as error:
            raise ProvenanceError(
                "explicit native build-tool override is unavailable"
            ) from error
        if not stat.S_ISREG(status.st_mode) or not os.access(resolved, os.X_OK):
            raise ProvenanceError(
                "explicit native build-tool override is not an executable"
            )
        fields[key] = hash_open_file(resolved, MAX_TOOL_BYTES)
    return sha256_bytes(encode_fields(fields))


def repository_identity(repo: Path) -> str:
    """Bind receipts to repository roots and the canonical common directory."""

    roots = git_output(repo, "rev-list", "--max-parents=0", "HEAD").splitlines()
    common_raw = git_output(repo, "rev-parse", "--git-common-dir")
    common = Path(common_raw)
    if not common.is_absolute():
        common = repo / common
    common = common.resolve(strict=True)
    fields = {
        "common": sha256_bytes(os.fsencode(common)),
        "roots": sha256_bytes("\n".join(sorted(roots)).encode()),
    }
    return sha256_bytes(encode_fields(fields))


def gate_script_digest(repo: Path, command: Sequence[str], env: dict[str, str]) -> str:
    """Hash the actual first gate executable selected by the normalized environment."""

    first = command[0]
    candidate = Path(first)
    if not candidate.is_absolute() and "/" in first:
        candidate = repo / candidate
    elif not candidate.is_absolute():
        located = shutil.which(first, path=env.get("PATH"))
        if not located:
            return sha256_bytes(os.fsencode(first))
        candidate = Path(located)
    return hash_open_file(candidate, MAX_REPOSITORY_FILE_BYTES, resolve=True)


def recipe_digest(repo: Path, env: dict[str, str]) -> str:
    try:
        recipe = run_checked(("just", "--show", "quality-gates"), cwd=repo, env=env)
    except ProvenanceError:
        recipe = b"quality-gates-recipe-missing"
    return sha256_bytes(recipe)


def collect_manifest(repo: Path, command: Sequence[str], env: dict[str, str]) -> bytes:
    """Collect the canonical acceptance manifest in a normalized child."""

    if not command:
        raise ProvenanceError("quality-gate command is empty")
    compiler, target = compiler_provenance(repo, env)
    launcher_invocation, launcher_authority, launcher_projection = (
        toolchain_launcher_provenance(env)
    )
    sandbox_index_clean = git_diff_is_clean(repo, env, "--cached")
    untracked = run_checked(
        ("git", "ls-files", "--others", "--exclude-standard", "-z"),
        cwd=repo,
        env=env,
    )
    index_clean = env.get("CSA_QUALITY_GATE_HOST_INDEX_CLEAN")
    tracked_clean = env.get("CSA_QUALITY_GATE_HOST_TRACKED_CLEAN")
    untracked_digest = env.get("CSA_QUALITY_GATE_HOST_UNTRACKED_SHA256")
    host_index_tree = env.get("CSA_QUALITY_GATE_HOST_INDEX_TREE")
    host_source = env.get("CSA_QUALITY_GATE_HOST_SOURCE_SHA256")
    if index_clean not in {"true", "false"} or tracked_clean not in {"true", "false"}:
        raise ProvenanceError("host clean-state provenance is unavailable")
    if untracked_digest is None or len(untracked_digest) != 64:
        raise ProvenanceError("host untracked provenance is unavailable")
    if not host_index_tree or not host_source or not is_lower_sha256(host_source):
        raise ProvenanceError("host source provenance is unavailable")
    if (index_clean == "true") != sandbox_index_clean:
        raise ProvenanceError("index clean-state changed during snapshot")
    sandbox_head_tree = git_output(repo, "rev-parse", "HEAD^{tree}")
    if index_clean == "true" and host_index_tree != sandbox_head_tree:
        raise ProvenanceError("index tree changed during snapshot")
    if untracked:
        raise ProvenanceError("sandbox snapshot contains unexpected untracked files")
    checkout = repo.resolve(strict=True)
    if not checkout.is_absolute():
        raise ProvenanceError("checkout provenance was not absolute")
    feature_matrix = env.get(
        "CSA_QUALITY_GATE_FEATURE_MATRIX",
        "workspace-default,workspace-all-features,e2e",
    )
    source_snapshot = env.get("CSA_QUALITY_GATE_SOURCE_SNAPSHOT_SHA256", "")
    sandbox_version = env.get("CSA_QUALITY_GATE_SANDBOX_VERSION", "")
    if not is_lower_sha256(source_snapshot) or not sandbox_version:
        raise ProvenanceError("sandbox provenance is unavailable")
    tracked_digests = {
        "cargo_lock_sha256": "Cargo.lock",
        "implementation_sha256": "scripts/hooks/quality-gate-receipt.sh",
        "justfile_sha256": "justfile",
        "lefthook_sha256": "lefthook.yml",
        "normalizer_sha256": "scripts/cargo-env-normalize.sh",
        "quality_gate_entrypoint_sha256": "scripts/hooks/quality-gates.sh",
        "quality_gate_environment_sha256": "scripts/quality_gate_environment.py",
        "quality_gate_host_attestation_sha256": (
            "scripts/quality_gate_host_attestation.py"
        ),
        "quality_gate_live_sha256": "scripts/hooks/quality-gates-live.sh",
        "quality_gate_process_sha256": "scripts/quality_gate_process.py",
        "quality_gate_provenance_sha256": "scripts/quality_gate_provenance.py",
        "quality_gate_sandbox_sha256": "scripts/quality_gate_sandbox.py",
        "quality_gate_secure_state_sha256": "scripts/quality_gate_secure_state.py",
        "quality_gate_state_helper_sha256": "scripts/quality-gate-state.py",
        "quality_gate_toolchain_sha256": "scripts/quality_gate_toolchain.py",
        "rust_toolchain_file_sha256": "rust-toolchain.toml",
        "weave_lock_sha256": "weave.lock",
    }
    fields = {
        "cargo_config_sha256": cargo_config_provenance(repo, env),
        "checkout_identity": sha256_bytes(os.fsencode(checkout)),
        "dotenv_sha256": dotenv_provenance(repo),
        "environment_sha256": environment_provenance(env),
        "feature_matrix_sha256": sha256_bytes(feature_matrix.encode()),
        "gate_command_sha256": command_digest(command),
        "gate_script_sha256": gate_script_digest(repo, command, env),
        "head_oid": git_output(repo, "rev-parse", "HEAD"),
        "host_tcb_policy": "ambient_os_runtime_v1",
        "implementation_version": IMPLEMENTATION_VERSION,
        "index_clean": index_clean,
        "index_tree_oid": host_index_tree,
        "index_oid": sha256_bytes(
            run_checked(("git", "ls-files", "--stage", "-z"), cwd=repo, env=env)
        ),
        "recipe_sha256": recipe_digest(repo, env),
        "repository_identity": repository_identity(repo),
        "rust_toolchain_launcher_authority_sha256": launcher_authority,
        "rust_toolchain_launcher_invocation_sha256": launcher_invocation,
        "rust_toolchain_semantic_projection": launcher_projection,
        "rust_toolchain_sha256": compiler,
        "schema_version": str(SCHEMA_VERSION),
        "sandbox_version": sandbox_version,
        "source_host_sha256": host_source,
        "source_snapshot_sha256": source_snapshot,
        "target_provenance_sha256": target,
        "tool_provenance_sha256": tool_provenance(repo, env),
        "tracked_worktree_clean": tracked_clean,
        "tree_oid": git_output(repo, "rev-parse", "HEAD^{tree}"),
        "untracked_worktree_digest": untracked_digest,
    }
    for key, relative in tracked_digests.items():
        fields[key] = optional_repository_digest(repo, relative)
    manifest = encode_fields(fields)
    if len(manifest) > MAX_MANIFEST_BYTES:
        raise ProvenanceError("acceptance manifest is too large")
    return manifest
