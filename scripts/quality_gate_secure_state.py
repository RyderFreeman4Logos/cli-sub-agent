"""Descriptor-owned hostile-state handling for quality-gate receipts.

The receipt digest detects accidental or partial content corruption.  It is not
publisher authentication against another same-UID process; the coordinator's
Linux capability boundary supplies that property by withholding this state.
"""

from __future__ import annotations

import ctypes
import errno
import fcntl
import hashlib
import json
import os
import secrets
import signal
import stat
import time
from dataclasses import dataclass
from pathlib import Path

__all__ = (
    "IMPLEMENTATION_VERSION",
    "SCHEMA_VERSION",
    "ReceiptValidation",
    "SecureState",
    "StateError",
    "sha256_bytes",
)

SCHEMA_VERSION = 2
IMPLEMENTATION_VERSION = "5"
LOCK_TIMEOUT_SECONDS = 2.0
LOCK_POLL_SECONDS = 0.05
MAX_RECEIPT_BYTES = 64 * 1024
RENAME_NOREPLACE = 1

DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
LOCK_FLAGS = os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
RECEIPT_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
CREATE_FLAGS = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC


class StateError(RuntimeError):
    """A hostile or unavailable receipt-state condition."""


@dataclass(frozen=True)
class ReceiptValidation:
    """Result of validating a receipt through its checked descriptor."""

    reason: str
    can_quarantine: bool = False


def sha256_bytes(value: bytes) -> str:
    """Return a lowercase SHA-256 digest."""

    return hashlib.sha256(value).hexdigest()


def open_directory_at(parent: int, name: str, *, create: bool, private: bool) -> int:
    """Open one directory component without following links or path re-resolution."""

    try:
        descriptor = os.open(name, DIRECTORY_FLAGS, dir_fd=parent)
    except FileNotFoundError:
        if not create:
            raise StateError("state directory is missing")
        try:
            os.mkdir(name, 0o700, dir_fd=parent)
        except FileExistsError:
            pass
        except OSError as error:
            raise StateError("state directory creation failed") from error
        try:
            descriptor = os.open(name, DIRECTORY_FLAGS, dir_fd=parent)
        except OSError as error:
            raise StateError("state directory open failed") from error
    except OSError as error:
        raise StateError("state directory open failed") from error
    try:
        status = os.fstat(descriptor)
        if not stat.S_ISDIR(status.st_mode) or status.st_uid != os.geteuid():
            raise StateError("state directory type or owner is unsafe")
        if stat.S_IMODE(status.st_mode) & 0o022:
            raise StateError("state directory is writable by another identity")
        if private and stat.S_IMODE(status.st_mode) != 0o700:
            os.fchmod(descriptor, 0o700)
            if stat.S_IMODE(os.fstat(descriptor).st_mode) != 0o700:
                raise StateError("state directory mode could not be made private")
        linked = os.stat(name, dir_fd=parent, follow_symlinks=False)
        current = os.fstat(descriptor)
        if (linked.st_dev, linked.st_ino) != (current.st_dev, current.st_ino):
            raise StateError("state directory link changed while opening")
        return descriptor
    except OSError as error:
        os.close(descriptor)
        raise StateError("state directory descriptor validation failed") from error
    except BaseException:
        os.close(descriptor)
        raise


class SecureState:
    """Descriptor-owned checkout-local quality-gate receipt state."""

    def __init__(self, descriptor: int) -> None:
        self.descriptor = descriptor

    @classmethod
    def open(cls, repo: Path) -> "SecureState":
        """Open or securely create ``.csa/state/quality-gate-receipts``."""

        try:
            repo_fd = os.open(repo, DIRECTORY_FLAGS)
        except OSError as error:
            raise StateError("repository root could not be opened securely") from error
        opened: list[int] = [repo_fd]
        try:
            csa = open_directory_at(repo_fd, ".csa", create=True, private=False)
            opened.append(csa)
            state = open_directory_at(csa, "state", create=True, private=True)
            opened.append(state)
            receipts = open_directory_at(
                state, "quality-gate-receipts", create=True, private=True
            )
            for descriptor in opened:
                os.close(descriptor)
            return cls(receipts)
        except BaseException:
            for descriptor in opened:
                os.close(descriptor)
            raise

    def close(self) -> None:
        """Close the owned state directory descriptor."""

        os.close(self.descriptor)

    def open_lock(self, name: str) -> int:
        """Open a private regular lock without truncating or following links."""

        try:
            descriptor = os.open(name, LOCK_FLAGS, 0o600, dir_fd=self.descriptor)
        except OSError as error:
            raise StateError("lock file open failed") from error
        try:
            status = os.fstat(descriptor)
            if (
                not stat.S_ISREG(status.st_mode)
                or status.st_uid != os.geteuid()
                or status.st_nlink != 1
                or stat.S_IMODE(status.st_mode) & 0o022
            ):
                raise StateError("lock file type, owner, links, or mode is unsafe")
            if stat.S_IMODE(status.st_mode) != 0o600:
                os.fchmod(descriptor, 0o600)
            linked = os.stat(name, dir_fd=self.descriptor, follow_symlinks=False)
            current = os.fstat(descriptor)
            if (linked.st_dev, linked.st_ino) != (current.st_dev, current.st_ino):
                raise StateError("lock file link changed while opening")
            return descriptor
        except OSError as error:
            os.close(descriptor)
            raise StateError("lock descriptor validation failed") from error
        except BaseException:
            os.close(descriptor)
            raise

    def acquire_lock(self, descriptor: int) -> bool:
        """Acquire an exclusive lock with a fixed monotonic deadline."""

        deadline = time.monotonic() + LOCK_TIMEOUT_SECONDS
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return True
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    return False
                time.sleep(
                    min(LOCK_POLL_SECONDS, max(0.0, deadline - time.monotonic()))
                )
            except OSError as error:
                raise StateError("lock acquisition failed") from error

    def validate_receipt(
        self, name: str, identity: str, manifest: bytes
    ) -> ReceiptValidation:
        """Validate a bounded receipt through the descriptor that supplied its bytes."""

        try:
            descriptor = os.open(name, RECEIPT_FLAGS, dir_fd=self.descriptor)
        except FileNotFoundError:
            return ReceiptValidation("receipt_missing")
        except OSError as error:
            if error.errno == errno.ELOOP:
                return ReceiptValidation("receipt_symlink")
            return ReceiptValidation("receipt_not_file")
        try:
            status = os.fstat(descriptor)
            if not stat.S_ISREG(status.st_mode):
                return ReceiptValidation("receipt_not_file")
            if status.st_uid != os.geteuid():
                return ReceiptValidation("receipt_owner_unsafe")
            if status.st_nlink != 1:
                return ReceiptValidation("receipt_hard_link")
            if stat.S_IMODE(status.st_mode) != 0o600:
                return ReceiptValidation("receipt_mode_unsafe")
            linked = os.stat(name, dir_fd=self.descriptor, follow_symlinks=False)
            if (linked.st_dev, linked.st_ino) != (status.st_dev, status.st_ino):
                return ReceiptValidation("receipt_link_changed")
            if status.st_size > MAX_RECEIPT_BYTES:
                return ReceiptValidation("receipt_too_large", True)
            content = bytearray()
            while len(content) <= status.st_size:
                chunk = os.read(
                    descriptor, min(8192, status.st_size + 1 - len(content))
                )
                if not chunk:
                    break
                content.extend(chunk)
            if len(content) != status.st_size or os.read(descriptor, 1):
                return ReceiptValidation("receipt_size_changed", True)
        except OSError:
            return ReceiptValidation("receipt_io_error")
        finally:
            os.close(descriptor)
        try:
            receipt = json.loads(
                bytes(content).decode("utf-8"),
                object_pairs_hook=reject_duplicate_json_keys,
            )
        except (UnicodeError, ValueError, json.JSONDecodeError):
            return ReceiptValidation("receipt_malformed", True)
        required = {
            "identity",
            "implementation_version",
            "manifest",
            "manifest_sha256",
            "receipt_digest",
            "schema_version",
            "status",
        }
        if not isinstance(receipt, dict) or set(receipt) != required:
            return ReceiptValidation("receipt_fields_invalid", True)
        if receipt["schema_version"] != SCHEMA_VERSION:
            return ReceiptValidation("receipt_schema_unknown", True)
        if receipt["implementation_version"] != IMPLEMENTATION_VERSION:
            return ReceiptValidation("receipt_implementation_stale", True)
        if receipt["status"] != "PASS":
            return ReceiptValidation("receipt_status_not_pass", True)
        try:
            stored_manifest = receipt["manifest"].encode("utf-8")
        except (AttributeError, UnicodeError):
            return ReceiptValidation("receipt_manifest_mismatch", True)
        if (
            receipt["identity"] != identity
            or receipt["manifest_sha256"] != identity
            or sha256_bytes(manifest) != identity
            or stored_manifest != manifest
        ):
            return ReceiptValidation("receipt_manifest_mismatch", True)
        payload = {key: receipt[key] for key in sorted(required - {"receipt_digest"})}
        expected = sha256_bytes(
            json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
        )
        if receipt["receipt_digest"] != expected:
            return ReceiptValidation("receipt_content_digest_mismatch", True)
        return ReceiptValidation("valid")

    def quarantine(self, name: str) -> bool:
        """Move a previously descriptor-validated regular receipt aside atomically."""

        rejected = f"rejected.{secrets.token_hex(16)}"
        try:
            rename_no_replace(self.descriptor, name, self.descriptor, rejected)
            os.fsync(self.descriptor)
            return True
        except OSError:
            return False

    def publish(self, name: str, identity: str, manifest: bytes) -> bool:
        """Publish one content-checked receipt by atomic no-replace rename.

        This method assumes the caller has excluded untrusted same-UID children
        from the state capability; the unkeyed digest alone cannot establish
        which process authored a receipt.
        """

        payload: dict[str, object] = {
            "identity": identity,
            "implementation_version": IMPLEMENTATION_VERSION,
            "manifest": manifest.decode("utf-8"),
            "manifest_sha256": identity,
            "schema_version": SCHEMA_VERSION,
            "status": "PASS",
        }
        payload["receipt_digest"] = sha256_bytes(
            json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
        )
        content = (
            json.dumps(payload, sort_keys=True, separators=(",", ":")).encode() + b"\n"
        )
        if len(content) > MAX_RECEIPT_BYTES:
            return False
        temporary = f".receipt.{identity}.{secrets.token_hex(16)}.tmp"
        try:
            descriptor = os.open(temporary, CREATE_FLAGS, 0o600, dir_fd=self.descriptor)
        except OSError:
            return False
        try:
            offset = 0
            while offset < len(content):
                offset += os.write(descriptor, content[offset:])
            os.fsync(descriptor)
        except OSError:
            os.close(descriptor)
            safe_unlink(self.descriptor, temporary)
            return False
        os.close(descriptor)
        if os.environ.get("CSA_QUALITY_GATE_TEST_FAULT") == "crash-before-publish":
            os.kill(os.getpid(), signal.SIGKILL)
        try:
            rename_no_replace(self.descriptor, temporary, self.descriptor, name)
            os.fsync(self.descriptor)
            return True
        except FileExistsError:
            return self.validate_receipt(name, identity, manifest).reason == "valid"
        except OSError:
            return False
        finally:
            safe_unlink(self.descriptor, temporary)


def reject_duplicate_json_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    """Reject duplicate JSON object keys rather than accepting the last value."""

    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def rename_no_replace(
    source_fd: int, source: str, destination_fd: int, destination: str
) -> None:
    """Call Linux renameat2 with RENAME_NOREPLACE and checked descriptors."""

    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = libc.renameat2
    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    if (
        renameat2(
            source_fd,
            os.fsencode(source),
            destination_fd,
            os.fsencode(destination),
            RENAME_NOREPLACE,
        )
        == 0
    ):
        return
    error_number = ctypes.get_errno()
    if error_number == errno.EEXIST:
        raise FileExistsError(error_number, os.strerror(error_number), destination)
    raise OSError(error_number, os.strerror(error_number), source)


def safe_unlink(directory: int, name: str) -> None:
    """Best-effort unlink of one known temporary entry relative to a secure dirfd."""

    try:
        os.unlink(name, dir_fd=directory)
    except FileNotFoundError:
        pass
    except OSError:
        pass
