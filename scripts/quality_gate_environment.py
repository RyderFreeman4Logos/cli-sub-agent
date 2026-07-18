"""Credential-free deterministic environment for a reusable static gate."""

from __future__ import annotations

from typing import Mapping

__all__ = ("PRIVATE_BIN_PATH", "normalized_static_environment")

PRIVATE_BIN_PATH = "/run/csa-bin"
SECRET_MARKERS = ("AUTH", "CREDENTIAL", "KEY", "PASSWORD", "SECRET", "TOKEN")
NETWORK_MARKERS = ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY")


def _contains_secret(name: str) -> bool:
    upper = name.upper()
    return any(marker in upper for marker in SECRET_MARKERS)


def normalized_static_environment(
    source: Mapping[str, str],
    source_fingerprint: str,
    clean_state: tuple[str, str, str],
) -> dict[str, str]:
    """Return the complete, scrubbed environment shared by identity and execution."""

    allowed: dict[str, str] = {}
    for name, value in source.items():
        if (
            _contains_secret(name)
            or name.upper() in NETWORK_MARKERS
            or name.startswith("CARGO_DENY_")
        ):
            continue
        if (
            name in {"HOME", "LOGNAME", "SHELL", "TERM", "TMPDIR", "USER"}
            or name.startswith(("CARGO_", "RUST", "NEXTEST_"))
            or name
            in {
                "AR",
                "BINDGEN_EXTRA_CLANG_ARGS",
                "CC",
                "CFLAGS",
                "CPP",
                "CPPFLAGS",
                "CXX",
                "CXXFLAGS",
                "LD",
                "LDFLAGS",
                "PKG_CONFIG_PATH",
                "CSA_PRESERVE_CARGO_TARGET_DIR",
                "CSA_QUALITY_GATE_FEATURE_MATRIX",
            }
        ):
            allowed[name] = value
    allowed.update(
        {
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_KEY_0": "core.excludesFile",
            "GIT_CONFIG_KEY_1": "core.attributesFile",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_SYSTEM": "/dev/null",
            "GIT_CONFIG_VALUE_0": "/dev/null",
            "GIT_CONFIG_VALUE_1": "/dev/null",
            "CARGO_NET_OFFLINE": "true",
            "LANG": "C",
            "LC_ALL": "C",
            "MISE_DATA_DIR": "/run/csa-mise-disabled",
            "PATH": f"{PRIVATE_BIN_PATH}:/usr/bin:/bin",
            "PYTHONDONTWRITEBYTECODE": "1",
            "CSA_QUALITY_GATE_SANDBOX_VERSION": "bwrap-static-v2",
            "CSA_QUALITY_GATE_SOURCE_SNAPSHOT_SHA256": source_fingerprint,
            "CSA_QUALITY_GATE_HOST_INDEX_CLEAN": clean_state[0],
            "CSA_QUALITY_GATE_HOST_TRACKED_CLEAN": clean_state[1],
            "CSA_QUALITY_GATE_HOST_UNTRACKED_SHA256": clean_state[2],
            "TZ": "UTC",
        }
    )
    allowed.setdefault("HOME", "/home/quality-gate")
    allowed.setdefault("TMPDIR", "/tmp")
    return allowed
