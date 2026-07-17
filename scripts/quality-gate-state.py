#!/usr/bin/env python3
"""Secure exact-input quality-gate receipt storage and provenance collection.

The public ``run`` command owns the whole receipt lifecycle. Acceptance inputs are
collected by a child entered through ``cargo-env-normalize.sh`` so the receipt sees
the same Rust/Cargo environment as the authoritative gate. Receipt state is opened
relative to verified directory descriptors and is never trusted by pathname alone.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
from pathlib import Path
from typing import Sequence

from quality_gate_provenance import (
    MAX_MANIFEST_BYTES,
    ProvenanceError,
    collect_manifest,
)
from quality_gate_secure_state import (
    SCHEMA_VERSION,
    SecureState,
    StateError,
    sha256_bytes,
)


def normalized_manifest(repo: Path, command: Sequence[str]) -> bytes:
    """Collect a manifest through the gate's exact Cargo normalization contract."""

    helper = repo / "scripts" / "quality-gate-state.py"
    normalizer = repo / "scripts" / "cargo-env-normalize.sh"
    try:
        completed = subprocess.run(
            (
                str(normalizer),
                sys.executable,
                str(helper),
                "collect",
                "--repo",
                str(repo),
                "--",
                *command,
            ),
            cwd=repo,
            env=os.environ.copy(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ProvenanceError("normalized manifest collection failed") from error
    if completed.returncode != 0 or len(completed.stdout) > MAX_MANIFEST_BYTES:
        raise ProvenanceError("normalized manifest collection failed")
    return completed.stdout


def manifest_fields(manifest: bytes) -> dict[str, str]:
    """Decode the internal canonical manifest after construction."""

    try:
        fields = dict(line.split("=", 1) for line in manifest.decode().splitlines())
    except (UnicodeError, ValueError) as error:
        raise ProvenanceError("internal manifest could not be decoded") from error
    return fields


def emit_result(
    status: str, identity: str, reason: str | None, code: int, manifest: bytes | None
) -> None:
    """Emit one path-safe structured result record."""

    fields = manifest_fields(manifest) if manifest else {}
    record = {
        "gate_exit_code": code,
        "provenance": {
            "checkout": fields.get("checkout_identity", "unavailable"),
            "head": fields.get("head_oid", "unavailable"),
            "repository": fields.get("repository_identity", "unavailable"),
        },
        "receipt_identity": identity,
        "rejection_reason": reason,
        "schema_version": SCHEMA_VERSION,
        "status": status,
    }
    print(json.dumps(record, sort_keys=True, separators=(",", ":")), flush=True)


def execute_gate(repo: Path, command: Sequence[str]) -> tuple[int, str | None]:
    """Run the gate without a shell and preserve signal-derived exit status."""

    active: subprocess.Popen[bytes] | None = None
    received_signal: signal.Signals | None = None

    def forward(signum: int, _frame: object) -> None:
        nonlocal received_signal
        received_signal = signal.Signals(signum)
        if active is not None and active.poll() is None:
            try:
                os.kill(active.pid, signum)
            except ProcessLookupError:
                pass

    previous = {
        signum: signal.signal(signum, forward)
        for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM)
    }
    try:
        try:
            active = subprocess.Popen(
                list(command),
                cwd=repo,
                stdin=None,
                stdout=sys.stderr,
                stderr=sys.stderr,
            )
        except OSError:
            return 127, "gate_exec_failed"
        return_code = active.wait()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    if received_signal is not None:
        return 128 + int(received_signal), {
            signal.SIGHUP: "signal_hup",
            signal.SIGINT: "signal_int",
            signal.SIGTERM: "signal_term",
        }[received_signal]
    if return_code < 0:
        return 128 - return_code, "gate_child_signal"
    return return_code, None


def run_uncached(
    repo: Path,
    command: Sequence[str],
    manifest: bytes | None,
    reason: str,
) -> int:
    """Fail closed by executing the full gate without consuming or publishing state."""

    identity = sha256_bytes(manifest) if manifest else "0" * 64
    code, signal_reason = execute_gate(repo, command)
    if code != 0:
        emit_result(
            "gate_failed",
            identity,
            signal_reason or "gate_exit_nonzero",
            code,
            manifest,
        )
        return code
    emit_result("executed", identity, reason, 0, manifest)
    return 0


def run_receipt_gate(repo: Path, command: Sequence[str]) -> int:
    """Execute or reuse the full quality gate for exact normalized inputs."""

    manifest: bytes | None = None
    state: SecureState | None = None
    collection_lock: int | None = None
    identity_lock: int | None = None
    try:
        fallback_reason: str | None = None
        try:
            state = SecureState.open(repo)
            collection_lock = state.open_lock("collection.lock")
            if not state.acquire_lock(collection_lock):
                fallback_reason = "lock_timeout"
            else:
                manifest = normalized_manifest(repo, command)
                identity = sha256_bytes(manifest)
                identity_lock = state.open_lock(f"{identity}.lock")
                if not state.acquire_lock(identity_lock):
                    fallback_reason = "lock_timeout"
        except StateError:
            if manifest is None:
                try:
                    manifest = normalized_manifest(repo, command)
                except ProvenanceError:
                    pass
            fallback_reason = "state_untrusted"
        except ProvenanceError:
            fallback_reason = "provenance_invalid"
        finally:
            if collection_lock is not None:
                os.close(collection_lock)
                collection_lock = None

        if fallback_reason is not None:
            if identity_lock is not None:
                os.close(identity_lock)
                identity_lock = None
            return run_uncached(repo, command, manifest, fallback_reason)

        if manifest is None or state is None:
            return run_uncached(repo, command, manifest, "provenance_invalid")
        identity = sha256_bytes(manifest)
        name = f"{identity}.json"
        validation = state.validate_receipt(name, identity, manifest)
        if validation.reason == "valid":
            emit_result("reused", identity, None, 0, manifest)
            return 0

        can_publish = validation.reason == "receipt_missing"
        if validation.can_quarantine:
            can_publish = state.quarantine(name)

        code, signal_reason = execute_gate(repo, command)
        if code != 0:
            emit_result(
                "gate_failed",
                identity,
                signal_reason or "gate_exit_nonzero",
                code,
                manifest,
            )
            return code
        try:
            post_manifest = normalized_manifest(repo, command)
        except ProvenanceError:
            emit_result("executed", identity, "input_drift", 0, manifest)
            return 0
        if post_manifest != manifest:
            emit_result("executed", identity, "input_drift", 0, manifest)
            return 0

        fields = manifest_fields(manifest)
        if (
            fields.get("index_clean") != "true"
            or fields.get("tracked_worktree_clean") != "true"
            or fields.get("untracked_worktree_digest") != sha256_bytes(b"")
        ):
            emit_result("executed", identity, "dirty_state", 0, manifest)
            return 0
        if not can_publish:
            emit_result("executed", identity, validation.reason, 0, manifest)
            return 0
        if not state.publish(name, identity, manifest):
            emit_result("gate_failed", identity, "publication_failed", 1, manifest)
            return 1
        emit_result("executed", identity, validation.reason, 0, manifest)
        return 0
    finally:
        if identity_lock is not None:
            os.close(identity_lock)
        if collection_lock is not None:
            os.close(collection_lock)
        if state is not None:
            state.close()


def parse_repository(value: str) -> Path:
    """Validate the repository root as an existing canonical absolute path."""

    path = Path(value)
    if not path.is_absolute():
        raise ProvenanceError("repository root must be absolute")
    resolved = path.resolve(strict=True)
    if resolved != path or not resolved.is_dir():
        raise ProvenanceError("repository root must be canonical")
    return resolved


def parser() -> argparse.ArgumentParser:
    """Build the small two-command CLI."""

    root = argparse.ArgumentParser(description=__doc__)
    subcommands = root.add_subparsers(dest="subcommand", required=True)
    for name in ("run", "collect"):
        command = subcommands.add_parser(name)
        command.add_argument("--repo", required=True)
        command.add_argument("gate", nargs=argparse.REMAINDER)
    return root


def normalize_gate_arguments(arguments: Sequence[str]) -> list[str]:
    """Strip argparse's separator and reject an empty gate command."""

    gate = list(arguments)
    if gate and gate[0] == "--":
        gate.pop(0)
    if not gate:
        raise ProvenanceError("quality-gate command is empty")
    return gate


def main() -> int:
    """Dispatch the public receipt runner or normalized collector."""

    arguments = parser().parse_args()
    try:
        repo = parse_repository(arguments.repo)
        gate = normalize_gate_arguments(arguments.gate)
        if arguments.subcommand == "collect":
            sys.stdout.buffer.write(collect_manifest(repo, gate, os.environ.copy()))
            return 0
        return run_receipt_gate(repo, gate)
    except ProvenanceError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
