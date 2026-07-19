"""Credential-free deterministic environment for a reusable static gate."""

from __future__ import annotations

from typing import Mapping

__all__ = ("PRIVATE_BIN_PATH", "normalized_static_environment")

PRIVATE_BIN_PATH = "/run/csa-bin"
SECRET_MARKERS = ("AUTH", "CREDENTIAL", "KEY", "PASSWORD", "SECRET", "TOKEN")
# Whole-variable network/credential drops (QGR-005): a name like
# CARGO_HTTP_PROXY contains none of the SECRET_MARKERS yet carries a URL
# that may embed credentials and is meaningless to an offline static gate.
NETWORK_VAR_NAMES = {
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "FTP_PROXY",
    "CARGO_HTTP_PROXY",
    "CARGO_NET_GIT_FETCH_WITH_CLI",
    "RUSTUP_DIST_SERVER",
    "RUSTUP_UPDATE_ROOT",
}
# Variables that carry a registry URL or index whose value may embed secrets
# and is irrelevant to an offline static build.
REGISTRY_CREDENTIAL_VARS = {
    "CARGO_REGISTRY_INDEX",
    "CARGO_REGISTRY_DEFAULT",
    "CARGO_REGISTRIES_CRATES_IO_PROTOCOL",
    "CARGO_REGISTRY_TOKEN",
    "CARGO_NET_GIT_FETCH_WITH_CLI",
}
# Ambient host identity/path variables (QGR-003). Forwarded verbatim is
# non-deterministic across hosts; pin them to constants below.
AMBIENT_PINNED = ("HOME", "LOGNAME", "SHELL", "TERM", "TMPDIR", "USER")
AMBIENT_PINNED_VALUES = {
    "HOME": "/home/quality-gate",
    "LOGNAME": "quality-gate",
    "SHELL": "/bin/sh",
    "TERM": "dumb",
    "TMPDIR": "/tmp",
    "USER": "quality-gate",
}


def _is_network_or_credential(name: str) -> bool:
    upper = name.upper()
    if any(marker in upper for marker in SECRET_MARKERS):
        return True
    if upper in NETWORK_VAR_NAMES:
        return True
    if upper in REGISTRY_CREDENTIAL_VARS:
        return True
    # Registry tokens: CARGO_REGISTRIES_<NAME>_TOKEN, CARGO_REGISTRIES_<NAME>_INDEX
    # when the value is a URL may carry credentials; treat the TOKEN form as secret
    # and the INDEX/PROTOCOL form as network.
    if upper.startswith("CARGO_REGISTRIES_") and (
        upper.endswith("_TOKEN") or upper.endswith("_INDEX") or upper.endswith("_PROTOCOL")
    ):
        return True
    return False


def normalized_static_environment(
    source: Mapping[str, str],
    projection_fingerprint: str,
    host_source_fingerprint: str,
    clean_state: tuple[str, str, str, str],
) -> dict[str, str]:
    """Return scrubbed execution inputs plus separate host/projection attestations."""

    allowed: dict[str, str] = {}
    for name, value in source.items():
        if _is_network_or_credential(name):
            continue
        if name.startswith("CARGO_DENY_"):
            continue
        if (
            name in AMBIENT_PINNED
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
    # Pin ambient identity/path variables to deterministic constants.
    for name, value in AMBIENT_PINNED_VALUES.items():
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
            "JUST_TEMPDIR": "/tmp",
            "LANG": "C",
            "LC_ALL": "C",
            "MISE_DATA_DIR": "/run/csa-mise-disabled",
            "PATH": f"{PRIVATE_BIN_PATH}:/usr/bin:/bin",
            "PYTHONDONTWRITEBYTECODE": "1",
            "CSA_QUALITY_GATE_SANDBOX_VERSION": "bwrap-static-v3",
            "CSA_QUALITY_GATE_SOURCE_SNAPSHOT_SHA256": projection_fingerprint,
            "CSA_QUALITY_GATE_HOST_SOURCE_SHA256": host_source_fingerprint,
            "CSA_QUALITY_GATE_HOST_INDEX_CLEAN": clean_state[0],
            "CSA_QUALITY_GATE_HOST_TRACKED_CLEAN": clean_state[1],
            "CSA_QUALITY_GATE_HOST_UNTRACKED_SHA256": clean_state[2],
            "CSA_QUALITY_GATE_HOST_INDEX_TREE": clean_state[3],
            "TZ": "UTC",
        }
    )
    return allowed
