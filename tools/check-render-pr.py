"""Select the renderer checks and validate the comparison supplied with a PR."""

import json
import math
import os
import re
import subprocess
import sys
from decimal import Decimal
from pathlib import Path, PurePosixPath


def affects_rendering(path):
    name = PurePosixPath(path)
    if name.suffix.lower() in {".md", ".rst"} or path in {"LICENSE", ".gitignore"}:
        return False
    if path.startswith(".github/ISSUE_TEMPLATE/"):
        return False
    # Include new runtime crates, fixtures and launch profiles by default.
    return True


def check_evidence(pr):
    body = re.sub(r"<!--.*?-->", "", pr.get("body") or "", flags=re.S)

    def field(label):
        match = re.search(r"^" + re.escape(label) + r":[ \t]*([^\r\n]*)", body, re.M)
        if not match or not match[1].strip():
            raise ValueError(f"Fill in '{label}' in the PR description.")
        return match[1].strip()

    for label, ref in [("Baseline commit", "base"), ("Candidate commit", "head")]:
        sha = field(label).strip("`").lower()
        if not re.fullmatch(r"[0-9a-f]{7,40}", sha) or not pr[ref]["sha"].startswith(sha):
            raise ValueError(f"'{label}' must identify the current PR {ref} commit.")
    field("Scene and settings")
    if not re.search(r"https://[^\s<>)]+", field("Visual comparison")):
        raise ValueError("Link the baseline and candidate images or comparison report using HTTPS.")
    number = r"(\d+(?:\.\d+)?)"
    times = re.fullmatch(fr"{number}\s*/\s*{number}\s*->\s*{number}\s*/\s*{number}",
                         field("Frame times (median/p95 ms)"))
    if not times:
        raise ValueError("Enter frame times as baseline median/p95 -> candidate median/p95 in ms.")
    before, before_p95, after, after_p95 = map(Decimal, times.groups())
    if not all(math.isfinite(value) for value in (before, before_p95, after, after_p95)):
        raise ValueError("Frame times must be finite.")
    if not (0 < before <= before_p95 and 0 < after <= after_p95):
        raise ValueError("Frame times must be positive and p95 must be at least the median.")
    if after > before * Decimal("1.10") or after_p95 > before_p95 * Decimal("1.10"):
        raise ValueError("Median or p95 frame time regressed by more than 10% versus baseline.")
    if not re.search(r"^- \[[xX]\] I verified that this change does not regress 3D rendering "
                     r"or frame pacing\.[ \t]*\r?$", body, re.M):
        raise ValueError("Resolve regressions and check the rendering verification box.")


def main():
    run = True
    if os.environ["GITHUB_EVENT_NAME"] == "pull_request":
        pr = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))["pull_request"]
        # Actions checks out the test merge. Its first parent is the event's
        # base; disabling rename detection examines both sides of a rename.
        names = subprocess.check_output(
            ["git", "diff", "--no-renames", "--name-only", "-z", pr["base"]["sha"], "HEAD", "--"]
        )
        run = any(affects_rendering(os.fsdecode(name)) for name in names.split(b"\0") if name)
        if run:
            check_evidence(pr)
        else:
            print("Only documentation or repository metadata changed.")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"render={str(run).lower()}", file=output)


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        sys.exit(str(error))
