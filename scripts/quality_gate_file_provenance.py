"""Bounded file and compiler-closure provenance helpers."""

from __future__ import annotations

import hashlib
import os
import stat
from pathlib import Path

from quality_gate_secure_state import sha256_bytes

__all__ = ("ProvenanceError", "hash_open_file", "toolchain_closure_provenance")

MAX_TOOLCHAIN_ENTRIES = 4096
TOOLCHAIN_CONTENT_HASH_LIMIT = 1024 * 1024


class ProvenanceError(RuntimeError):
    """Acceptance input normalization."""


def hash_open_file(path: Path, maximum: int, *, resolve: bool = False) -> str:
    """Hash a bounded regular file, no final-component follow."""

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


def toolchain_closure_provenance(sysroot: Path) -> str:
    """Bind compiler closure: metadata for large libs, content for manifests."""

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