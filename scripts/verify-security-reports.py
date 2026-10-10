"""Require complete, nonempty scan coverage as well as zero unaccepted findings in CI."""
import json
import sys
from pathlib import Path


def verify(semgrep_path, bandit_path):
    semgrep = json.loads(Path(semgrep_path).read_text(encoding="utf-8-sig"))
    bandit = json.loads(Path(bandit_path).read_text(encoding="utf-8-sig"))
    for name, report in (("semgrep", semgrep), ("bandit", bandit)):
        if report.get("errors") != [] or report.get("results") != []:
            raise ValueError(f"{name}: errors or findings remain, or required fields are missing")
    scanned = semgrep.get("paths", {}).get("scanned")
    lines = bandit.get("metrics", {}).get("_totals", {}).get("loc")
    if not isinstance(scanned, list) or not scanned:
        raise ValueError("semgrep: no scanned files")
    if not isinstance(lines, int) or isinstance(lines, bool) or lines <= 0:
        raise ValueError("bandit: no analyzed lines")
    return {"semgrep_scanned_files": len(scanned), "bandit_analyzed_lines": lines}


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: verify-security-reports.py SEMGREP_JSON BANDIT_JSON")
    print(json.dumps(verify(sys.argv[1], sys.argv[2])))
