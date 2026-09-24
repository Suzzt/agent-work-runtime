#!/usr/bin/env python3
"""Materialize WS-051 business deliverable directories from fixtures."""
from __future__ import annotations

import json
import sys

from support import materialize_deliverable


def main() -> int:
    summary = materialize_deliverable()
    print(json.dumps({"ok": True, **summary}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
