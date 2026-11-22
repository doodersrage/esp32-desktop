#!/usr/bin/env python3
"""The library sweep's passes, for esp32bench.dev's project pages.

Reads a sweep's report.json (tests/library/sweep.py) and prints the projects whose
simulated builds printed their expected output, with the boards they did so on, as the
JSON the site keeps in src/data/desktop-sim.json:

    python3 tests/library/site_runs.py build/sweeps/r14/report.json --date 2026-11-19 \\
        > <esp32-bench>/src/data/desktop-sim.json

Only passes are listed: a build that crashed, hung or was not simulated says nothing.
"""

import argparse
import json
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("report", type=Path)
    ap.add_argument("--date", required=True, help="when the sweep ran, YYYY-MM-DD")
    a = ap.parse_args()
    report = json.loads(a.report.read_text())
    runs: dict[str, list[str]] = {}
    for key, entry in sorted(report.items()):
        project, board = key.split("/", 1)
        if (entry.get("boot") or {}).get("result") == "pass":
            runs.setdefault(project, []).append(board)
    print(json.dumps({"checked": a.date, "runs": runs}, indent=1, sort_keys=True))


if __name__ == "__main__":
    main()
