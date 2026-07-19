#!/usr/bin/env python3
"""Validate the exact Static/Live nextest partition and focused test guards."""

from __future__ import annotations

import argparse
import json
import tempfile
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

EXPECTED_LIVE = frozenset(
    {
        (
            "cli-sub-agent::skill_resource_inheritance",
            "skill_run_preserves_plan_parent_resource_snapshot_for_nested_child",
        ),
        (
            "cli-sub-agent::bin/csa",
            "pipeline::clean_room_integration_tests::clean_room_executes_admitted_fake_and_leaves_only_minimal_session_artifacts",
        ),
        (
            "cli-sub-agent::bin/csa",
            "review_cmd::review_convergence::clean_room_provider_tests::production_adapter_executes_only_fingerprinted_fake_with_exact_contract",
        ),
        (
            "cli-sub-agent::bin/csa",
            "review_cmd::review_convergence::clean_room_provider_tests::production_adapter_propagates_fingerprinted_fake_nonzero_exit",
        ),
    }
)
CLAUSE = re.compile(
    r"\(binary_id\(=([A-Za-z0-9_:/.-]+)\) & test\(=([A-Za-z0-9_:.-]+)\)\)"
)
FUNCTION_START = r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:async[ \t]+)?fn[ \t]+{name}[ \t]*\("


class ContractError(ValueError):
    """A fail-closed partition contract violation."""


@dataclass(frozen=True)
class Inventory:
    universe: frozenset[tuple[str, str]]
    matches: frozenset[tuple[str, str]]


def require_mapping(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ContractError(f"{label} must be an object")
    return value


def load_selector(config_path: Path) -> frozenset[tuple[str, str]]:
    try:
        with config_path.open("rb") as handle:
            config = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ContractError(f"invalid nextest config: {error}") from error
    profile = require_mapping(config.get("profile"), "profile")
    static = require_mapping(profile.get("static"), "profile.static")
    selector = static.get("default-filter")
    if not isinstance(selector, str):
        raise ContractError("profile.static.default-filter must be a string")
    live = profile.get("live")
    if isinstance(live, dict) and "default-filter" in live:
        raise ContractError("profile.live.default-filter is a forbidden second authority")
    if not selector.startswith("not (") or not selector.endswith(")"):
        raise ContractError("static selector must be one canonical not (...) complement")
    body = selector[5:-1]
    raw_clauses = body.split(" | ")
    if len(raw_clauses) != 4:
        raise ContractError(f"static selector must contain exactly 4 clauses, got {len(raw_clauses)}")
    tuples: list[tuple[str, str]] = []
    for raw_clause in raw_clauses:
        match = CLAUSE.fullmatch(raw_clause)
        if match is None:
            raise ContractError(f"malformed or factorized selector clause: {raw_clause}")
        tuples.append((match.group(1), match.group(2)))
    selected = frozenset(tuples)
    if len(selected) != len(tuples):
        raise ContractError("static selector contains a duplicate tuple")
    if selected != EXPECTED_LIVE:
        missing = sorted(EXPECTED_LIVE - selected)
        unknown = sorted(selected - EXPECTED_LIVE)
        raise ContractError(f"static selector tuple drift: missing={missing} unknown={unknown}")
    return selected


def test_selector_fixtures(args: argparse.Namespace) -> None:
    load_selector(args.config)
    clauses = [
        f"(binary_id(={binary_id}) & test(={test_name}))"
        for binary_id, test_name in sorted(EXPECTED_LIVE)
    ]
    old_false_live = (
        "(binary_id(=csa-executor) & "
        "test(=transport::tests::test_execute_best_effort_sandbox_fallback_preserves_attempt_model_override))"
    )
    unknown = "(binary_id(=unknown) & test(=unknown::test))"
    selector_cases = {
        "three": "not (" + " | ".join(clauses[:3]) + ")",
        "five": "not (" + " | ".join([*clauses, clauses[0]]) + ")",
        "old-false-live": "not (" + " | ".join([old_false_live, *clauses[1:]]) + ")",
        "duplicate": "not (" + " | ".join([clauses[0], clauses[0], *clauses[2:]]) + ")",
        "unknown": "not (" + " | ".join([unknown, *clauses[1:]]) + ")",
        "broad": "not (binary_id(=cli-sub-agent::bin/csa))",
        "factorized": "not ((binary_id(=cli-sub-agent::bin/csa) & (test(=a) | test(=b))))",
    }
    canonical = args.config.read_text(encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="live-selector-contract-") as raw_root:
        root = Path(raw_root)
        for name, selector in selector_cases.items():
            path = root / f"{name}.toml"
            path.write_text(
                f'[profile.static]\ndefault-filter = "{selector}"\n', encoding="utf-8"
            )
            try:
                load_selector(path)
            except ContractError:
                continue
            raise ContractError(f"selector fixture unexpectedly accepted: {name}")
        for name, body in {
            "missing": "[profile.static]\nretries = 0\n",
            "second-authority": canonical
            + '\n[profile.live]\ndefault-filter = "all()"\n',
        }.items():
            path = root / f"{name}.toml"
            path.write_text(body, encoding="utf-8")
            try:
                load_selector(path)
            except ContractError:
                continue
            raise ContractError(f"selector fixture unexpectedly accepted: {name}")


def emit_fixture_inventory(args: argparse.Namespace) -> None:
    nextest_args = args.nextest_args
    mode = (
        "live"
        if "not default()" in nextest_args
        else "all"
        if "--ignore-default-filter" in nextest_args
        else "static"
    )
    all_features = "--all-features" in nextest_args
    live = sorted(EXPECTED_LIVE)
    static = [("csa-executor", "transport::tests::static_fixture")]
    identities = [*live, *static]
    if args.fault == "identity-drift" and mode == "live" and all_features:
        identities[0] = (identities[0][0], identities[0][1] + "_drift")
    suites: dict[str, dict[str, object]] = {}
    for binary_id, test_name in identities:
        is_live = (binary_id, test_name) in live
        is_static = (binary_id, test_name) in static
        status = "matches" if mode == "all" or (mode == "live" and is_live) or (mode == "static" and is_static) else "mismatch"
        if args.fault == "live-3" and mode == "live" and (binary_id, test_name) == live[0]:
            status = "mismatch"
        if args.fault == "live-5" and mode == "live" and is_static:
            status = "matches"
        if args.fault == "overlap" and mode == "static" and (binary_id, test_name) == live[0]:
            status = "matches"
        if args.fault == "union-omission" and mode in {"static", "live"} and (binary_id, test_name) == live[0]:
            status = "mismatch"
        if args.fault == "unknown-status" and mode == "live" and (binary_id, test_name) == live[0]:
            status = "unknown"
        suite = suites.setdefault(binary_id, {"binary-id": binary_id, "testcases": {}})
        testcases = suite["testcases"]
        assert isinstance(testcases, dict)
        testcases[test_name] = {"filter-match": {"status": status}}
    print(json.dumps({"rust-suites": suites}, sort_keys=True))


def load_inventory(path: Path, label: str) -> Inventory:
    try:
        inventory = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"invalid {label} inventory: {error}") from error
    suites = require_mapping(require_mapping(inventory, label).get("rust-suites"), f"{label}.rust-suites")
    universe: set[tuple[str, str]] = set()
    matches: set[tuple[str, str]] = set()
    for suite_name, raw_suite in suites.items():
        suite = require_mapping(raw_suite, f"{label}.rust-suites[{suite_name!r}]")
        binary_id = suite.get("binary-id")
        if not isinstance(binary_id, str) or not binary_id:
            raise ContractError(f"{label} suite {suite_name!r} has invalid binary-id")
        testcases = require_mapping(suite.get("testcases"), f"{label}.{binary_id}.testcases")
        for test_name, raw_testcase in testcases.items():
            if not isinstance(test_name, str) or not test_name:
                raise ContractError(f"{label}.{binary_id} has an invalid testcase name")
            testcase = require_mapping(raw_testcase, f"{label}.{binary_id}.{test_name}")
            filter_match = require_mapping(
                testcase.get("filter-match"), f"{label}.{binary_id}.{test_name}.filter-match"
            )
            status = filter_match.get("status")
            if status not in {"matches", "mismatch"}:
                raise ContractError(
                    f"{label}.{binary_id}.{test_name} has unknown filter status {status!r}"
                )
            identity = (binary_id, test_name)
            if identity in universe:
                raise ContractError(f"{label} contains duplicate identity {identity}")
            universe.add(identity)
            if status == "matches":
                matches.add(identity)
    return Inventory(frozenset(universe), frozenset(matches))


def validate_inventories(args: argparse.Namespace) -> None:
    expected = load_selector(args.config)
    all_inventory = load_inventory(args.all_inventory, f"{args.leg} All")
    static_inventory = load_inventory(args.static_inventory, f"{args.leg} Static")
    live_inventory = load_inventory(args.live_inventory, f"{args.leg} Live")
    if static_inventory.universe != all_inventory.universe:
        raise ContractError(f"{args.leg} Static inventory universe differs from All")
    overlap = static_inventory.matches & live_inventory.matches
    if overlap:
        raise ContractError(f"{args.leg} Static/Live overlap: {sorted(overlap)}")
    union = static_inventory.matches | live_inventory.matches
    if union != all_inventory.matches:
        raise ContractError(f"{args.leg} Static/Live union differs from All")
    if live_inventory.matches != expected or len(live_inventory.matches) != 4:
        raise ContractError(
            f"{args.leg} Live identities differ from canonical four: {sorted(live_inventory.matches)}"
        )
    args.identities_out.write_text(
        "".join(f"{binary_id}\t{test_name}\n" for binary_id, test_name in sorted(live_inventory.matches)),
        encoding="utf-8",
    )


def check_function(args: argparse.Namespace) -> None:
    try:
        source = args.source.read_text(encoding="utf-8")
    except OSError as error:
        raise ContractError(f"unable to read function source: {error}") from error
    start = re.search(FUNCTION_START.format(name=re.escape(args.function)), source)
    if start is None:
        raise ContractError(f"function not found: {args.function}")
    tail = source[start.start() :]
    next_item = re.search(r"(?m)^#\[(?:tokio::)?test(?:\([^]]*\))?\]", tail[1:])
    body = tail if next_item is None else tail[: next_item.start() + 1]
    if re.search(r"detect_(?:resource|filesystem)_capability[ \t]*\(", body):
        raise ContractError(f"{args.function} calls ambient capability detection")
    if re.search(r"(?m)^[ \t]*return(?:[ \t]+[^;]+)?;", body):
        raise ContractError(f"{args.function} contains a bare capability skip return")


def emit_selector(args: argparse.Namespace) -> None:
    for binary_id, test_name in sorted(load_selector(args.config)):
        print(f"{binary_id}\t{test_name}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    selector = subparsers.add_parser("selector")
    selector.add_argument("--config", type=Path, required=True)
    selector.set_defaults(handler=emit_selector)
    selector_fixtures = subparsers.add_parser("test-selector-fixtures")
    selector_fixtures.add_argument("--config", type=Path, required=True)
    selector_fixtures.set_defaults(handler=test_selector_fixtures)
    fixture_inventory = subparsers.add_parser("fixture-inventory")
    fixture_inventory.add_argument("--fault", default="none")
    fixture_inventory.add_argument("nextest_args", nargs=argparse.REMAINDER)
    fixture_inventory.set_defaults(handler=emit_fixture_inventory)
    inventories = subparsers.add_parser("validate-inventories")
    inventories.add_argument("--config", type=Path, required=True)
    inventories.add_argument("--leg", required=True)
    inventories.add_argument("--all", dest="all_inventory", type=Path, required=True)
    inventories.add_argument("--static", dest="static_inventory", type=Path, required=True)
    inventories.add_argument("--live", dest="live_inventory", type=Path, required=True)
    inventories.add_argument("--identities-out", type=Path, required=True)
    inventories.set_defaults(handler=validate_inventories)
    function = subparsers.add_parser("check-function")
    function.add_argument("--source", type=Path, required=True)
    function.add_argument("--function", required=True)
    function.set_defaults(handler=check_function)
    return parser


def main() -> int:
    try:
        args = build_parser().parse_args()
        args.handler(args)
    except ContractError as error:
        print(f"ERROR: live partition contract: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
