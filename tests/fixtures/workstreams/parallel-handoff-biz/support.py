#!/usr/bin/env python3
"""Shared helpers for workstream parallel-handoff BIZ acceptance (AWR-WS-051)."""
from __future__ import annotations

import hashlib
import json
import re
import shutil
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

HARD_GATES = [
    "three_way_parallel",
    "cross_line_dependency_rework",
    "cross_person_handoff_recovery",
    "same_person_agent_switch_subtask_parallel",
    "auth_revoke_and_reject",
    "time_forecast_stage_acceptance",
]

NAMED_CLIENTS = {"codex_cli", "claude_code"}
REQUIRED_SURFACES = {"codex_cli", "claude_code", "team_web"}

LEAK_PATTERNS = [
    ("internal_scenario_id", re.compile(r"(?<![0-9A-Za-z_-])WS-BIZ-[0-9]+(?![0-9A-Za-z_-])")),
    ("internal_work_id", re.compile(r"(?<![0-9A-Za-z_-])AWR-WS-051(?![0-9A-Za-z_-])")),
    ("fixture_path", re.compile(r"tests/fixtures/workstreams/parallel-handoff-biz")),
    ("evidence_path", re.compile(r"\.local/awr-workstream-implementation-20260921")),
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
    require(scenario["scope_revision"] == 2, "scope_revision must be 2")
    require(scenario["hard_gate"] == catalog_row["hard_gate"], "hard_gate mismatch")
    require(scenario["hard_gate"] in HARD_GATES, "unknown hard gate")
    require(scenario["team_web_required"] is True, "team web required")
    require(scenario["native_client_required"] is True, "native clients required")
    require(scenario["minimum_real_clients"] >= 2, "need >=2 real clients")
    require(set(scenario["named_clients"]) == NAMED_CLIENTS, "named clients")
    require(set(scenario["required_surfaces"]) == REQUIRED_SURFACES, "required surfaces")
    require(scenario["independent_reviewer_required"] is True, "independent reviewer required")
    validate_prompts(scenario, directory)

    identities = read_json(directory / scenario["identities"])
    delegations = read_json(directory / scenario["delegations"])
    persons = {p["person_id"]: p for p in identities["persons"]}
    bindings = scenario["person_bindings"]
    same_person = bool(scenario.get("same_person_dual_agent"))

    if same_person:
        require(bindings["executor"] == bindings["peer"], "same-person peer binding")
        require(len(persons) >= 2, "need owner + independent reviewer (+ optional observer)")
        require(scenario["minimum_developer_persons"] >= 1, "same-person min developers")
    else:
        require(bindings["executor"] != bindings["peer"], "two developer persons")
        require(len(persons) >= 3, "need >=2 developers + reviewer")
        require(scenario["minimum_developer_persons"] >= 2, "need >=2 developer persons")

    require(bindings["executor"] != bindings["reviewer"], "reviewer independent")
    if not same_person:
        require(bindings["peer"] != bindings["reviewer"], "reviewer independent of peer")
    for key in ("executor", "peer", "reviewer"):
        require(bindings[key] in persons, f"missing person {key}")
    require(bindings.get("web_operator") in persons, "web_operator person")

    review_dels = [d for d in delegations["delegations"] if d.get("independent_review")]
    require(len(review_dels) == 1, "exactly one independent review delegation")
    require(review_dels[0]["person_id"] == bindings["reviewer"], "reviewer delegation person")
    require(review_dels[0]["client_adapter"] == "team_web", "reviewer uses team_web")

    adapters = {d["client_adapter"] for d in delegations["delegations"]}
    require(NAMED_CLIENTS.issubset(adapters), "both named clients delegated")
    require("team_web" in adapters, "team_web delegation")

    if same_person:
        owner_dels = [d for d in delegations["delegations"] if d["person_id"] == bindings["executor"]]
        require(len({d["client_adapter"] for d in owner_dels} & NAMED_CLIENTS) == 2, "owner dual agents")

    project = directory / scenario["project_directory"]
    require(project.is_dir(), "project directory")
    manifest = tomllib.loads((project / "project.toml").read_text(encoding="utf-8"))
    require(manifest["project"]["external_key"] == scenario["namespace"], "project external_key")
    for name in ("GOALS.md", "PLAN.md", "RULES.md", "work-ledger.json"):
        require((project / name).is_file(), f"missing {name}")
    rules = (project / "RULES.md").read_text(encoding="utf-8")
    require(rules.count("severity=hard scope=project value=*") >= 2, "hard rules")
    require("sql" in rules.lower(), "forbid SQL mutation")
    require("web" in rules.lower(), "web surface in rules")

    ledger = read_json(project / "work-ledger.json")
    graph = read_json(directory / scenario["work_graph"])
    require(graph["namespace"] == scenario["namespace"], "graph namespace")
    require(graph["hard_gate"] == scenario["hard_gate"], "graph hard_gate")
    require(set(graph["required_surfaces"]) == REQUIRED_SURFACES, "graph surfaces")
    rows = {r["id"]: r for r in ledger["work_items"]}
    nodes = {n["key"]: n for n in graph["nodes"]}
    require(set(rows) == set(nodes), f"ledger/graph mismatch {set(rows) ^ set(nodes)}")
    require(scenario["entry_work"] in rows, "entry work missing")
    require(all(r.get("owner") is None for r in rows.values()), "seeded owners forbidden")
    require(any(n["role"] == "reviewer" for n in nodes.values()), "reviewer node")
    require(any(n["client_adapter"] == "team_web" for n in nodes.values()), "team_web node")
    require(any(n["client_adapter"] == "codex_cli" for n in nodes.values()), "codex_cli node")
    require(any(n["client_adapter"] == "claude_code" for n in nodes.values()), "claude_code node")
    require(bindings["reviewer"] in {n["person_id"] for n in nodes.values()}, "reviewer on graph")

    if same_person:
        non_review = [n for n in nodes.values() if n["role"] != "reviewer" and n["role"] != "web_operator"]
        require(all(n["person_id"] == bindings["executor"] for n in non_review if n["role"] in {"executor", "executor_peer"}), "same person on dual agents")
        require(len({n["client_adapter"] for n in non_review if n["role"] in {"executor", "executor_peer"}}) >= 2, "dual agent adapters")
    else:
        require(any(n["role"] == "executor_peer" for n in nodes.values()), "peer node")
        require(
            len({n["person_id"] for n in nodes.values() if n["role"] in {"executor", "executor_peer"}}) >= 2,
            "two developer persons on graph",
        )

    if scenario["hard_gate"] == "three_way_parallel":
        # FE/BE ready in parallel (no mutual deps) plus a web QA join
        roots = [n for n in nodes.values() if not n["depends_on"] and n["role"] in {"executor", "executor_peer"}]
        require(len(roots) >= 2, "three-way needs parallel roots")
        require(any(n["role"] == "web_operator" for n in nodes.values()), "qa/web lane")

    for art in scenario["artifacts"]:
        require(art["path"].startswith("deliverables/"), "artifact path")
        require(not (project / art["path"]).exists(), "artifacts must not be seeded")
    require(any(a["producer"] == "reviewer" for a in scenario["artifacts"]), "reviewer artifact")
    require(len(scenario["followups"]) == 2, "two followups")
    for fu in scenario["followups"]:
        for pub in fu["publish"]:
            src = directory / pub["source"]
            require(src.is_file() and src.stat().st_size > 0, f"missing update {pub['source']}")
            require(pub["source"].startswith(f"updates/round-{fu['number']}/"), "update path")
    require(scenario["artifact_directory"] == catalog_row["artifact_directory"], "artifact dir")
    require(scenario["api_db_evidence_policy"] == "read_only", "read_only evidence")
    return {
        "work_keys": sorted(rows),
        "persons": sorted(persons),
        "clients": sorted(scenario["named_clients"]),
        "surfaces": sorted(scenario["required_surfaces"]),
        "hard_gate": scenario["hard_gate"],
    }


def load_bundle():
    catalog = read_json(BASE / "catalog.json")
    contract = read_json(BASE / "contract.json")
    require(contract["required_gates"] == GATES, "contract gates")
    require(contract["scope_revision"] == 2, "contract scope_revision")
    require(len(catalog["scenarios"]) == contract["target_scenarios"] == 6, "six scenarios")
    require(set(catalog["hard_gates"]) == set(HARD_GATES), "hard gates inventory")
    specs = {}
    namespaces = set()
    work_keys = set()
    hard_seen = set()
    for row in catalog["scenarios"]:
        directory = ROOT / row["fixture"]
        scenario = read_json(directory / "scenario.json")
        result = validate_scenario(scenario, directory, row)
        require(row["namespace"] not in namespaces, "namespace reuse")
        require(not work_keys.intersection(result["work_keys"]), "work key reuse")
        require(row["hard_gate"] not in hard_seen, "hard gate reuse")
        namespaces.add(row["namespace"])
        work_keys.update(result["work_keys"])
        hard_seen.add(row["hard_gate"])
        specs[row["id"]] = (scenario, directory, result)
    require(hard_seen == set(HARD_GATES), "all hard gates covered")
    return catalog, contract, specs


def materialize_deliverable(evidence_root: Path | None = None) -> dict:
    """Copy fixtures into the WS-051 deliverable layout under .local/.../business/scope-r2."""
    catalog, contract, specs = load_bundle()
    root = evidence_root or (ROOT / contract["evidence_root"])
    root.mkdir(parents=True, exist_ok=True)
    summary = {"scenarios": {}, "evidence_root": str(root.relative_to(ROOT))}
    for sid, (scenario, directory, result) in specs.items():
        slug = directory.name
        out = root / slug
        fixture_dst = out / "fixture"
        if fixture_dst.exists():
            shutil.rmtree(fixture_dst)
        shutil.copytree(directory, fixture_dst)
        # Canonical work-graph + empty artifacts dir + report template copy
        shutil.copy2(directory / "work-graph.json", out / "work-graph.json")
        (out / "artifacts").mkdir(exist_ok=True)
        (out / "artifacts" / ".gitkeep").write_text("", encoding="utf-8")
        report_path = out / "report.json"
        if not report_path.exists():
            template = read_json(directory / "report.template.json")
            report_path.write_text(json.dumps(template, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        summary["scenarios"][sid] = {
            "fixture": str(fixture_dst.relative_to(ROOT)),
            "work_graph": str((out / "work-graph.json").relative_to(ROOT)),
            "artifacts": str((out / "artifacts").relative_to(ROOT)),
            "report": str(report_path.relative_to(ROOT)),
            "hard_gate": result["hard_gate"],
            "persons": result["persons"],
            "surfaces": result["surfaces"],
        }
    index = {
        "work": "AWR-WS-051",
        "scope_revision": 2,
        "contract_id": contract["contract_id"],
        "status": "blocked_pending_trial_participants",
        "scenarios": summary["scenarios"],
    }
    (root / "index.json").write_text(json.dumps(index, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    summary["index"] = str((root / "index.json").relative_to(ROOT))
    return summary
