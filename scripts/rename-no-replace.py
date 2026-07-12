#!/usr/bin/python3
"""Atomically rename one filesystem entry without replacing the destination."""

from __future__ import annotations

import ctypes
import errno
import os
import sys

AT_FDCWD = -100
RENAME_NOREPLACE = 1


def main() -> int:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <source> <destination>", file=sys.stderr)
        return 2

    source = os.fsencode(sys.argv[1])
    destination = os.fsencode(sys.argv[2])
    libc = ctypes.CDLL(None, use_errno=True)
    try:
        renameat2 = libc.renameat2
    except AttributeError:
        print(
            "ERROR: atomic no-replace rename is unavailable on this platform",
            file=sys.stderr,
        )
        return 2

    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    if renameat2(AT_FDCWD, source, AT_FDCWD, destination, RENAME_NOREPLACE) == 0:
        return 0

    error_number = ctypes.get_errno()
    if error_number == errno.EEXIST:
        print(
            f"ERROR: destination already exists: {os.fsdecode(destination)}",
            file=sys.stderr,
        )
        return 3
    raise OSError(error_number, os.strerror(error_number), os.fsdecode(source))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except OSError as error:
        print(f"ERROR: atomic no-replace rename failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
