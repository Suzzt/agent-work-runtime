#!/usr/bin/env python3
"""Consistency checks for DEC-011 signal fixtures (no product side effects)."""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def load(name: str):
    return json.loads((ROOT / name).read_text())


def main() -> int:
    manifest = load("manifest.json")
    schema = load("schema.json")
    assert manifest["schema_id"] == schema["schema_id"]
    assert manifest["dec_012_started"] is True
    for name in manifest["fixtures"]:
        data = load(name)
        assert "case" in data, name
    missing = load("missing-git-diff.json")
    assert missing["expect"]["workspace.changed_lines"] == "missing"
    assert missing["expect"]["workspace.change_risk"] == "unsupported"
    assert missing["forbidden"]["changed_lines_equals_zero"] is False
    conflict = load("version-conflict.json")
    assert conflict["expect"]["reject"] is True
    for item in schema["forbidden_coercions"]:
        assert "not_to" in item
    print("DEC-011 signal fixtures OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
