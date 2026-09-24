#!/usr/bin/env python3
"""Team-compatible local planning validator (AWR-EVO-002).

Discriminates contract type+version instead of treating every extension as an
awr-v1 work_items list. Unknown types stay rejected. Old V1/Team denominators
are read-only — this module never rewrites them.

Planning/governance validation only; not product runtime acceptance.
"""
from __future__ import annotations

import argparse
import json
import re
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable

try:
    import yaml
except ImportError as exc:  # pragma: no cover
    raise SystemExit(
        "FAIL: PyYAML required; use .venv/bin/python after installing requirements"
    ) from exc


EVO002_STATS_SCHEMA = "awr-evo-002-plan-validator-stats/v1"

CONTRACT_KIND: dict[tuple[str, str], str] = {
    ("awr-v1", "1.0.0"): "personal_v1_work_scope",
    ("awr-v1", "1.3.0"): "personal_v1_work_scope",
    ("awr-team-v1", "1.0.0"): "team_work_scope",
    ("awr-team-acceptance-v1", "1.0.0"): "team_acceptance_scenario",
}

GOAL_ID_RE = re.compile(r"^AWR-G-[A-Z0-9-]+$")

REQUIRED_V1_WORK_FIELDS = (
    "id",
    "title",
    "milestone",
    "priority",
    "status",
    "depends_on",
    "deliverables",
    "acceptance",
    "source_sections",
    "next_action",
    "required_for_v1",
    "verification",
)


class UniqueLoader(yaml.SafeLoader):
    pass


def _unique_mapping(loader, node, deep=False):
    mapping = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            raise ValueError(f"duplicate YAML key: {key}")
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


UniqueLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _unique_mapping
)


@dataclass
class Finding:
    code: str
    message: str


@dataclass
class ValidationReport:
    ok: bool
    mode: str
    findings: list[Finding] = field(default_factory=list)
    stats: dict[str, Any] = field(default_factory=dict)
    contracts_validated: list[dict[str, str]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "ok": self.ok,
            "mode": self.mode,
            "findings": [asdict(f) for f in self.findings],
            "stats": self.stats,
            "contracts_validated": self.contracts_validated,
        }


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_json(path: Path) -> Any:
    def pairs(values):
        result = {}
        for key, value in values:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    return json.loads(path.read_text(), object_pairs_hook=pairs)


def load_yaml(path: Path) -> Any:
    return yaml.load(path.read_text(), Loader=UniqueLoader)


def by_id(items: list[dict], label: str, id_key: str = "id") -> dict[str, dict]:
    result: dict[str, dict] = {}
    for item in items:
        require(id_key in item, f"{label} missing {id_key}")
        key = item[id_key]
        require(key not in result, f"duplicate {label} ID: {key}")
        result[key] = item
    return result


def resolve_kind(contract_id: str, version: str) -> str:
    key = (contract_id, str(version))
    if key not in CONTRACT_KIND:
        known = ", ".join(sorted(f"{c}@{v}" for c, v in CONTRACT_KIND))
        raise ValueError(
            f"unknown contract type/version: {contract_id}@{version} (known: {known})"
        )
    return CONTRACT_KIND[key]


def validate_personal_v1(contract: dict, section: dict, *, root: Path) -> dict:
    require(contract.get("contract_id") == "awr-v1", "personal_v1 binding requires awr-v1")
    version = str(contract.get("version"))
    require(version in ("1.0.0", "1.3.0"), f"unsupported awr-v1 version: {version}")
    require(
        contract.get("kind") in (None, "personal_v1_work_scope"),
        "awr-v1 kind mismatch",
    )
    if "work_items" not in section:
        raise ValueError("work_items")

    items = by_id(section["work_items"], "work item")
    misplaced = [i for i in items if GOAL_ID_RE.match(i)]
    if misplaced:
        raise ValueError(
            "misplaced_goal_in_work_items: "
            + ",".join(misplaced)
            + " (AWR-G-* belongs to goals; keep generation-phase unchanged unless "
            "original contract proves a legitimate source change)"
        )

    for key, item in items.items():
        missing = [f for f in REQUIRED_V1_WORK_FIELDS if f not in item]
        if missing:
            raise ValueError(f"{key}: missing required work fields: {', '.join(missing)}")
        require(
            isinstance(item["acceptance"], list) and item["acceptance"],
            f"{key}: empty acceptance",
        )
        require(
            len(set(item["acceptance"])) == len(item["acceptance"]),
            f"{key}: duplicate acceptance",
        )
        deps = item["depends_on"]
        require(isinstance(deps, list), f"{key}: depends_on must be list")
        require(
            len(deps) == len(set(deps)) and set(deps) <= set(items),
            f"{key}: invalid dependency",
        )
        if item["status"] == "completed":
            evidence = item.get("evidence") or []
            require(evidence, f"{key}: forged completion (completed without evidence)")
            acceptance_records = item.get("acceptance_results")
            if acceptance_records is not None:
                passed = {
                    a["criterion"]
                    for a in acceptance_records
                    if a.get("passed") is True and a.get("criterion")
                }
                require(
                    set(item["acceptance"]) <= passed,
                    f"{key}: forged completion (acceptance evidence incomplete)",
                )

    scope = contract.get("scope") or {}
    required_ids = list(scope.get("required_work_item_ids") or [])
    prep_ids = list(scope.get("preparation_work_item_ids") or [])
    require(len(required_ids) == len(set(required_ids)), "duplicate V1 scope ID")
    require(len(prep_ids) == len(set(prep_ids)), "duplicate preparation ID")
    if required_ids or prep_ids:
        require(
            not (set(required_ids) & set(prep_ids)),
            "overlapping required/preparation scope",
        )
        require(
            set(items) == set(required_ids) | set(prep_ids),
            "work scope differs from contract",
        )

    targets = scope.get("targets") or {}
    stats = {
        "kind": "personal_v1_work_scope",
        "contract_id": "awr-v1",
        "version": version,
        "work_items": len(items),
        "v1_denominator": targets.get("v1_work_items"),
        "preparation_denominator": targets.get("preparation_work_items"),
    }
    if targets.get("v1_work_items") is not None:
        counted = sum(1 for w in items.values() if w.get("required_for_v1"))
        require(
            counted == targets["v1_work_items"],
            "V1 work target count mismatch (denominator preserved; ledger must match)",
        )
    _ = root
    return stats


def validate_team_work_scope(contract: dict, section: dict, *, root: Path) -> dict:
    require(contract.get("contract_id") == "awr-team-v1", "team work-scope binding mismatch")
    version = str(contract.get("version"))
    require(version == "1.0.0", f"unsupported awr-team-v1 version: {version}")
    require(contract.get("kind") == "team_work_scope", "awr-team-v1 kind must be team_work_scope")

    if "work_items" in section and "work_definitions" not in section:
        raise ValueError(
            "wrong_binding: awr-team-v1 expects work_definitions, not awr-v1 work_items"
        )
    if "acceptance_scenarios" in section and "work_definitions" not in section:
        raise ValueError(
            "wrong_binding: awr-team-v1 is not an acceptance-scenario contract"
        )
    if "work_definitions" not in section:
        raise ValueError("work_items")

    defs = by_id(section["work_definitions"], "team work definition", id_key="work_id")
    required_fields = (
        "work_id",
        "external_key",
        "acceptance",
        "completion_policy",
        "scope_paths",
    )
    for key, item in defs.items():
        missing = [f for f in required_fields if f not in item]
        require(
            not missing,
            f"{key}: missing team work-scope fields: {', '.join(missing)}",
        )
        require(item["acceptance"], f"{key}: empty acceptance")
        if item.get("status") == "completed":
            receipt = item.get("completion_receipt")
            require(
                receipt,
                f"{key}: forged completion (completed without completion_receipt)",
            )
            require(
                receipt.get("verified") is True and receipt.get("receipt_id"),
                f"{key}: forged completion (receipt not verified)",
            )

    scope = contract.get("scope") or {}
    required_ids = list(scope.get("required_work_ids") or [])
    require(len(required_ids) == len(set(required_ids)), "duplicate team work-scope ID")
    if required_ids:
        require(set(defs) == set(required_ids), "team work scope differs from contract")

    targets = scope.get("targets") or {}
    stats = {
        "kind": "team_work_scope",
        "contract_id": "awr-team-v1",
        "version": version,
        "work_definitions": len(defs),
        "team_work_denominator": targets.get("team_work_items"),
    }
    if targets.get("team_work_items") is not None:
        require(
            len(defs) == targets["team_work_items"],
            "team work target count mismatch (denominator preserved; ledger must match)",
        )
    _ = root
    return stats


def validate_team_acceptance(contract: dict, section: dict, *, root: Path) -> dict:
    require(
        contract.get("contract_id") == "awr-team-acceptance-v1",
        "team acceptance binding mismatch",
    )
    version = str(contract.get("version"))
    require(version == "1.0.0", f"unsupported awr-team-acceptance-v1 version: {version}")
    require(
        contract.get("kind") == "team_acceptance_scenario",
        "awr-team-acceptance-v1 kind must be team_acceptance_scenario",
    )

    if "work_items" in section:
        raise ValueError(
            "wrong_binding: awr-team-acceptance-v1 must not be read as work_items"
        )
    if "work_definitions" in section:
        raise ValueError(
            "wrong_binding: awr-team-acceptance-v1 is not a team work-scope contract"
        )
    if "acceptance_scenarios" not in section:
        raise ValueError("work_items")

    scenarios = by_id(section["acceptance_scenarios"], "team acceptance scenario")
    for key, state in scenarios.items():
        for field_name in ("id", "title", "status", "required_gates"):
            require(field_name in state, f"{key}: missing {field_name}")
        require(
            len(state["required_gates"]) == len(set(state["required_gates"])),
            f"{key}: duplicate gate",
        )
        if state["status"] == "completed":
            gates = state.get("gates") or {}
            require(
                set(gates) == set(state["required_gates"])
                and all(v is True for v in gates.values()),
                f"{key}: forged completion (gates incomplete)",
            )
            require(
                state.get("executor")
                and state.get("reviewer")
                and state["executor"] != state["reviewer"],
                f"{key}: forged completion (independent review missing)",
            )
            require(
                state.get("evidence"),
                f"{key}: forged completion (no acceptance evidence)",
            )

    scope = contract.get("scope") or {}
    required_ids = list(scope.get("required_scenario_ids") or [])
    require(len(required_ids) == len(set(required_ids)), "duplicate acceptance scenario ID")
    if required_ids:
        require(set(scenarios) == set(required_ids), "acceptance scenario scope mismatch")

    targets = scope.get("targets") or {}
    stats: dict[str, Any] = {
        "kind": "team_acceptance_scenario",
        "contract_id": "awr-team-acceptance-v1",
        "version": version,
        "acceptance_scenarios": len(scenarios),
        "team_acceptance_denominator": targets.get("acceptance_scenarios"),
    }
    if targets.get("acceptance_scenarios") is not None:
        require(
            len(scenarios) == targets["acceptance_scenarios"],
            "acceptance scenario target count mismatch (denominator preserved)",
        )
    preserved = contract.get("preserved_team_v1_denominators")
    if preserved is not None:
        stats["preserved_team_v1_denominators"] = dict(preserved)
    _ = root
    return stats


VALIDATORS: dict[str, Callable[..., dict]] = {
    "personal_v1_work_scope": validate_personal_v1,
    "team_work_scope": validate_team_work_scope,
    "team_acceptance_scenario": validate_team_acceptance,
}


def load_contract(root: Path, binding: dict) -> dict:
    path = binding.get("path")
    if path:
        contract_path = (root / path).resolve()
        require(
            contract_path.is_relative_to(root.resolve()),
            f"path escapes repository: {path}",
        )
        require(contract_path.is_file(), f"missing contract: {path}")
        return load_json(contract_path)
    require("inline_contract" in binding, "binding needs path or inline_contract")
    return binding["inline_contract"]


def section_for_binding(ledger: dict, binding: dict, kind: str) -> dict:
    section_key = binding.get("section")
    if section_key:
        require(section_key in ledger, f"missing ledger section: {section_key}")
        section = ledger[section_key]
        require(isinstance(section, dict), f"ledger section {section_key} must be a mapping")
        return section

    extensions = ledger.get("extensions") or []
    if extensions:
        matches = [
            ext
            for ext in extensions
            if ext.get("contract_id") == binding["contract_id"]
            and str(ext.get("version")) == str(binding["version"])
        ]
        require(
            len(matches) == 1,
            f"extension binding not unique for {binding['contract_id']}@{binding['version']}",
        )
        return matches[0]

    if kind == "personal_v1_work_scope" and ledger.get("contract_id") == binding["contract_id"]:
        return ledger

    raise ValueError(
        f"cannot locate section for {binding['contract_id']}@{binding['version']} "
        f"(kind={kind}); provide binding.section or ledger.extensions"
    )


def normalize_error(error: BaseException) -> str:
    if isinstance(error, KeyError) and error.args == ("work_items",):
        return "work_items"
    msg = str(error) if str(error) else repr(error)
    if msg in ("work_items", "'work_items'"):
        return "work_items"
    return msg


def validate_book(
    root: Path, ledger: dict, bindings: list[dict] | None = None
) -> ValidationReport:
    stats_blocks: list[dict] = []
    validated: list[dict[str, str]] = []

    if bindings is None:
        bindings = list(ledger.get("contract_bindings") or [])
        if not bindings and ledger.get("contract_id"):
            bindings = [
                {
                    "contract_id": ledger["contract_id"],
                    "version": ledger.get("contract_version") or ledger.get("version"),
                    "path": f"contracts/{ledger['contract_id']}.json",
                }
            ]

    if not bindings:
        return ValidationReport(
            ok=False,
            mode="full_book",
            findings=[Finding("no_bindings", "FAIL: no contract bindings to validate")],
        )

    seen: set[tuple[str, str]] = set()
    try:
        for binding in bindings:
            cid = binding["contract_id"]
            ver = str(binding["version"])
            require((cid, ver) not in seen, f"duplicate contract binding: {cid}@{ver}")
            seen.add((cid, ver))

            kind = resolve_kind(cid, ver)
            contract = load_contract(root, binding)
            require(contract.get("contract_id") == cid, f"contract_id mismatch in {cid}")
            require(str(contract.get("version")) == ver, f"contract version mismatch in {cid}")
            expected_kind = CONTRACT_KIND[(cid, ver)]
            if contract.get("kind") and contract["kind"] != expected_kind:
                raise ValueError(
                    f"wrong_binding: contract {cid} declares kind={contract['kind']} "
                    f"but registry expects {expected_kind}"
                )

            section = section_for_binding(ledger, binding, kind)
            stats_blocks.append(VALIDATORS[kind](contract, section, root=root))
            validated.append({"contract_id": cid, "version": ver, "kind": kind})
    except (ValueError, KeyError, TypeError, OSError, yaml.YAMLError) as error:
        msg = normalize_error(error)
        return ValidationReport(
            ok=False,
            mode="full_book",
            findings=[
                Finding(
                    "validation_error",
                    msg if msg.startswith("FAIL:") else f"FAIL: {msg}",
                )
            ],
            stats={
                "schema": EVO002_STATS_SCHEMA,
                "blocks": stats_blocks,
                "independent_of_v1_team_denominators": True,
            },
            contracts_validated=validated,
        )

    return ValidationReport(
        ok=True,
        mode="full_book",
        findings=[],
        stats={
            "schema": EVO002_STATS_SCHEMA,
            "blocks": stats_blocks,
            "bindings": len(validated),
            "independent_of_v1_team_denominators": True,
            "note": "EVO-002 stats are independent; V1/Team denominators are not rewritten",
        },
        contracts_validated=validated,
    )


def validate_root(root: Path) -> ValidationReport:
    root = root.resolve()
    ledger_path = root / "ledger/work-ledger.yaml"
    if not ledger_path.is_file():
        return ValidationReport(
            ok=False,
            mode="full_book",
            findings=[
                Finding(
                    "ledger_missing",
                    "FAIL: missing ledger/work-ledger.yaml "
                    "(Mac-authoritative planning book not mounted on this box)",
                )
            ],
            stats={
                "schema": EVO002_STATS_SCHEMA,
                "independent_of_v1_team_denominators": True,
            },
        )
    return validate_book(root, load_yaml(ledger_path))


def validate_fixture_dir(fixture_root: Path) -> ValidationReport:
    fixture_root = fixture_root.resolve()
    ledger_path = fixture_root / "ledger.yaml"
    require(ledger_path.is_file(), f"missing {ledger_path}")
    return validate_book(fixture_root, load_yaml(ledger_path))


def naive_require_work_items(section: dict) -> None:
    """Historical defect: every extension treated as awr-v1 work_items list."""
    items = section["work_items"]
    by_id(items, "work item")


def reproduce_work_items_fail(fixture_root: Path) -> str:
    fixture_root = fixture_root.resolve()
    ledger = load_yaml(fixture_root / "ledger.yaml")
    try:
        if ledger.get("extensions"):
            for ext in ledger["extensions"]:
                naive_require_work_items(ext)
        else:
            naive_require_work_items(ledger)
    except Exception as error:  # noqa: BLE001
        if isinstance(error, KeyError) and error.args == ("work_items",):
            return "FAIL: 'work_items'"
        return f"FAIL: {error}"
    return "UNEXPECTED_PASS"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=None)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--reproduce-naive", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)

    if args.reproduce_naive:
        require(args.fixture is not None, "--reproduce-naive requires --fixture")
        print(reproduce_work_items_fail(args.fixture))
        return 1

    if args.fixture:
        report = validate_fixture_dir(args.fixture)
    else:
        root = (args.root or Path(__file__).resolve().parents[2]).resolve()
        report = validate_root(root)

    if args.json:
        print(json.dumps(report.to_dict(), ensure_ascii=False, indent=2))
    elif report.ok:
        kinds = ", ".join(
            f"{c['contract_id']}@{c['version']}" for c in report.contracts_validated
        )
        print(f"PASS: planning validation ok; contracts=[{kinds}]")
        print(
            "EVO-002 independent stats only; V1/Team denominators not rewritten. "
            "Planning validation only; no runtime/E4 claim."
        )
    else:
        for finding in report.findings:
            print(
                finding.message
                if finding.message.startswith("FAIL:")
                else f"FAIL: {finding.message}"
            )
    return 0 if report.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
