#!/usr/bin/env python3
"""Shared helpers for Team MCP BIZ acceptance fixtures (AWR-TMCP-051)."""
from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore

ROOT = Path(__file__).resolve().parents[4]
BASE = Path(__file__).resolve().parent

GATES = [
    "natural_client_initiation",
    "actual_tool_or_role_execution",
    "real_artifact",
    "business_followup_1",
    "business_followup_2",
    "independent_review",
    "delivery",
    "traceable_receipt",
]

LEAK_PATTERNS = [
    ("internal_scenario_id", re.compile(r"(?<![0-9A-Za-z_-])TMCP-BIZ-[0-9]+(?![0-9A-Za-z_-])")),
    ("internal_work_id", re.compile(r"(?<![0-9A-Za-z_-])AWR-TMCP-051(?![0-9A-Za-z_-])")),
    ("fixture_path", re.compile(r"tests/fixtures/team-mcp/biz-acceptance")),
    ("evidence_path", re.compile(r"\.local/awr-team-mcp-acceptance-v1")),
    (
        "gate_id_leak",
        re.compile(r"natural_client_initiation|traceable_receipt|actual_tool_or_role_execution"),
    ),
]


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def require(cond, msg):
    if not cond:
        raise AssertionError(msg)


def _scalar(v: str):
    v = v.strip()
    if v == "null":
        return None
    if v == "true":
        return True
    if v == "false":
        return False
    if v == "[]":
        return []
    if v.startswith("[") and v.endswith("]"):
        inner = v[1:-1].strip()
        if not inner:
            return []
        return [_scalar(p.strip()) for p in inner.split(",")]
    if (v.startswith('"') and v.endswith('"')) or (v.startswith("'") and v.endswith("'")):
        return json.loads('"' + v[1:-1].replace("\\", "\\\\").replace('"', '\\"') + '"') if v.startswith("'") else json.loads(v)
    if re.fullmatch(r"-?\d+", v):
        return int(v)
    return v




def read_yaml_simple(path: Path):
    """Parse the fixture YAML subset (mappings + list-of-mappings + scalar lists)."""
    try:
        import yaml  # type: ignore

        data = yaml.safe_load(path.read_text(encoding="utf-8"))
        require(isinstance(data, dict), f"YAML root must be mapping: {path}")
        return data
    except ImportError:
        pass

    lines = path.read_text(encoding="utf-8").splitlines()
    root = {}
    # stack entries: (indent, container)
    stack = [(-1, root)]

    def parse_value(raw: str):
        return _scalar(raw.strip()) if raw.strip() != "" else None

    i = 0
    while i < len(lines):
        raw = lines[i]
        i += 1
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        line = raw.strip()

        if line.startswith("- "):
            # Pop only deeper frames so same-column list items stay under their key.
            while len(stack) > 1 and indent < stack[-1][0]:
                stack.pop()
            # If top is a dict created for a key that should be a list, reject.
            container = stack[-1][1]
            require(isinstance(container, list), f"list item without list container at {path}:{i}: {line}")
            rest = line[2:]
            if ": " in rest:
                k, v = rest.split(": ", 1)
                item = {k: parse_value(v)}
                container.append(item)
                stack.append((indent, item))
            elif rest.endswith(":"):
                item = {rest[:-1]: None}
                container.append(item)
                stack.append((indent, item))
            else:
                container.append(parse_value(rest))
            continue

        # mapping key / scalar
        while len(stack) > 1 and indent <= stack[-1][0]:
            stack.pop()
        container = stack[-1][1]
        require(isinstance(container, dict), f"mapping entry needs dict at {path}:{i}: {line}")

        if line.endswith(":") and ": " not in line:
            key = line[:-1]
            nxt = None
            for j in range(i, len(lines)):
                if lines[j].strip() and not lines[j].lstrip().startswith("#"):
                    nxt = lines[j]
                    break
            if nxt is not None and nxt.lstrip().startswith("-"):
                value = []
            else:
                value = {}
            container[key] = value
            stack.append((indent, value))
            continue

        if ": " in line:
            k, v = line.split(": ", 1)
            container[k] = parse_value(v)
            continue

        raise AssertionError(f"unparsed YAML line in {path}:{i}: {line}")

    require(isinstance(root, dict), f"YAML root must be mapping: {path}")
    return root



def client_input_violations(text: str):
    findings = []
    for code, pattern in LEAK_PATTERNS:
        for match in pattern.finditer(text):
            findings.append({
                "code": code,
                "match": match.group(0),
                "span": [match.start(), match.end()],
            })
    return findings


def validate_prompts(scenario: dict, directory: Path):
    texts = [scenario["initial_request"], *(f["request"] for f in scenario["followups"])]
    for text in texts:
        findings = client_input_violations(text)
        require(not findings, f"prompt leak: {findings}")
    initial = (directory / "prompts/initial.md").read_text(encoding="utf-8").strip()
    require(initial == scenario["initial_request"], "initial prompt mismatch")
    for n, fu in enumerate(scenario["followups"], 1):
        body = (directory / f"prompts/followup-{n}.md").read_text(encoding="utf-8").strip()
        require(body == fu["request"], f"followup-{n} prompt mismatch")



def validate_scenario(scenario: dict, directory: Path, catalog_row: dict):
    require(scenario["scenario_id"] == catalog_row["id"], "scenario id mismatch")
    require(scenario["namespace"] == catalog_row["namespace"], "namespace mismatch")
    require(scenario["required_gates"] == GATES, "gate inventory mismatch")
    require(scenario["native_mcp_required"] is True, "native MCP required")
    require(scenario["minimum_real_clients"] >= 2, "need >=2 real clients")
    require(scenario["minimum_developer_persons"] >= 2, "need >=2 developer persons")
    require(set(scenario["named_clients"]) == {"codex_cli", "claude_code"}, "named clients")
    require(scenario["independent_reviewer_required"] is True, "independent reviewer required")
    validate_prompts(scenario, directory)

    identities = read_json(directory / scenario["identities"])
    delegations = read_json(directory / scenario["delegations"])
    persons = {p["person_id"]: p for p in identities["persons"]}
    require(len(persons) >= 3, "need >=2 developers + reviewer")
    bindings = scenario["person_bindings"]
    require(bindings["executor"] != bindings["reviewer"], "reviewer independent")
    require(bindings["peer"] != bindings["reviewer"], "reviewer independent of peer")
    require(bindings["executor"] != bindings["peer"], "two developer persons")
    for key in ("executor", "peer", "reviewer"):
        require(bindings[key] in persons, f"missing person {key}")

    review_dels = [d for d in delegations["delegations"] if d.get("independent_review")]
    require(len(review_dels) == 1, "exactly one independent review delegation")
    require(review_dels[0]["person_id"] == bindings["reviewer"], "reviewer delegation person")

    project = directory / scenario["project_directory"]
    require(project.is_dir(), "project directory")
    manifest = tomllib.loads((project / "project.toml").read_text(encoding="utf-8"))
    require(manifest["project"]["external_key"] == scenario["namespace"], "project external_key")
    for name in ("GOALS.md", "PLAN.md", "RULES.md", "work-ledger.json"):
        require((project / name).is_file(), f"missing {name}")
    rules = (project / "RULES.md").read_text(encoding="utf-8")
    require(rules.count("severity=hard scope=project value=*") >= 2, "hard rules")
    require("sql" in rules.lower(), "forbid SQL mutation")

    ledger = read_json(project / "work-ledger.json")
    graph_path = directory / scenario["work_graph"]
    graph = read_json(graph_path) if graph_path.suffix == ".json" else read_yaml_simple(graph_path)
    require(graph["namespace"] == scenario["namespace"], "graph namespace")
    rows = {r["id"]: r for r in ledger["work_items"]}
    nodes = {n["key"]: n for n in graph["nodes"]}
    require(set(rows) == set(nodes), f"ledger/graph mismatch {set(rows) ^ set(nodes)}")
    require(scenario["entry_work"] in rows, "entry work missing")
    require(all(r.get("owner") is None for r in rows.values()), "seeded owners forbidden")
    require(set(graph.get("reviewer_distinct_from") or []) == {"executor", "executor_peer"}, "reviewer distinct from")
    require(any(n["role"] == "reviewer" for n in nodes.values()), "reviewer node")
    require(any(n["role"] == "executor_peer" for n in nodes.values()), "peer node")
    require(bindings["reviewer"] in {n["person_id"] for n in nodes.values()}, "assertion failed")
    require(
        len({n["person_id"] for n in nodes.values() if n["role"] != "reviewer"}) >= 2,
        "two developer persons on graph",
    )

    for art in scenario["artifacts"]:
        require(art["path"].startswith("deliverables/"), "artifact path")
        require(not (project / art["path"]).exists(), "artifacts must not be seeded")
    require(any(a["producer"] == "reviewer" for a in scenario["artifacts"]), "assertion failed")
    require(len(scenario["followups"]) == 2, "assertion failed")
    for fu in scenario["followups"]:
        for pub in fu["publish"]:
            src = directory / pub["source"]
            require(src.is_file() and src.stat().st_size > 0, f"missing update {pub['source']}")
            require(pub["source"].startswith(f"updates/round-{fu['number']}/"), "assertion failed")
    require(scenario["artifact_directory"] == catalog_row["artifact_directory"], "assertion failed")
    require(scenario["api_db_evidence_policy"] == "read_only", "assertion failed")
    return {
        "work_keys": sorted(rows),
        "persons": sorted(persons),
        "clients": sorted(scenario["named_clients"]),
    }


def load_bundle():
    catalog = read_json(BASE / "catalog.json")
    contract = read_json(BASE / "contract.json")
    require(contract["required_gates"] == GATES, "contract gates")
    require(len(catalog["scenarios"]) == contract["target_scenarios"] == 4, "four scenarios")
    specs = {}
    namespaces = set()
    work_keys = set()
    for row in catalog["scenarios"]:
        directory = ROOT / row["fixture"]
        scenario = read_json(directory / "scenario.json")
        result = validate_scenario(scenario, directory, row)
        require(row["namespace"] not in namespaces, "namespace reuse")
        require(not work_keys.intersection(result["work_keys"]), "work key reuse")
        namespaces.add(row["namespace"])
        work_keys.update(result["work_keys"])
        specs[row["id"]] = (scenario, directory, result)
    return catalog, contract, specs
