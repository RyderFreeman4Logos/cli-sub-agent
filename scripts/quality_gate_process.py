"""Bounded lifecycle supervision for a quality-gate sandbox process.

Bubblewrap owns the isolated PID namespace.  This module owns the outer
supervisor lifecycle: signal forwarding, a finite TERM grace period, forced
namespace teardown, and direct-child reaping before control returns.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

__all__ = ("ProcessResult", "collect_bounded", "execute_supervised")

TERM_GRACE_SECONDS = 2.0
KILL_GRACE_SECONDS = 5.0
COLLECT_TIMEOUT_SECONDS = 60
MAX_COLLECT_BYTES = 256 * 1024


@dataclass(frozen=True)
class ProcessResult:
    """Terminal result after the complete sandbox tree has disappeared."""

    code: int
    reason: str | None = None


def collect_bounded(command: Sequence[str], cwd: Path) -> bytes:
    """Run a provenance collector with bounded time and output."""

    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            env={},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=COLLECT_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError("sandboxed provenance collection failed") from error
    if completed.returncode != 0 or len(completed.stdout) > MAX_COLLECT_BYTES:
        raise RuntimeError("sandboxed provenance collection failed")
    return completed.stdout


def execute_supervised(
    command: Sequence[str], cwd: Path, *, inject_start_failure: bool = False
) -> ProcessResult:
    """Run one namespace supervisor and synchronously reap it on every signal."""

    if inject_start_failure:
        return ProcessResult(125, "isolation_start_failed")

    active: subprocess.Popen[bytes] | None = None
    received: signal.Signals | None = None

    def forward(signum: int, _frame: object) -> None:
        nonlocal received
        if received is None:
            received = signal.Signals(signum)
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
                command,
                cwd=cwd,
                env={},
                stdin=None,
                stdout=sys.stderr,
                stderr=sys.stderr,
                close_fds=True,
                start_new_session=True,
            )
        except OSError:
            return ProcessResult(125, "isolation_start_failed")
        if received is not None:
            forward(int(received), object())
        termination_deadline: float | None = None
        while True:
            try:
                return_code = active.wait(timeout=0.1)
                break
            except subprocess.TimeoutExpired:
                if received is None:
                    continue
                if termination_deadline is None:
                    termination_deadline = time.monotonic() + TERM_GRACE_SECONDS
                if time.monotonic() < termination_deadline:
                    continue
                try:
                    os.kill(active.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    return_code = active.wait(timeout=KILL_GRACE_SECONDS)
                except subprocess.TimeoutExpired:
                    return ProcessResult(125, "isolation_cleanup_failed")
                break
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    if received is not None:
        reason = {
            signal.SIGHUP: "signal_hup",
            signal.SIGINT: "signal_int",
            signal.SIGTERM: "signal_term",
        }[received]
        return ProcessResult(128 + int(received), reason)
    if return_code < 0:
        return ProcessResult(128 - return_code, "gate_child_signal")
    return ProcessResult(return_code)
