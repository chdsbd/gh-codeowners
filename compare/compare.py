# /// script
# requires-python = ">=3.12"
# dependencies = ["codeowners==0.9.0"]
# ///
"""Compare CODEOWNERS libraries with GitHub on tests/github.txt.

Runs codeowners-stats, codeowners-rs, hmarr/codeowners (Go), sbdchd/codeowners
(Python) and the extension's @snyk/github-codeowners fork (Node) over every
recorded case, then prints how often each agrees with GitHub and the cases
where it doesn't. Needs cargo, go and npm on PATH; a library whose tool is
missing is skipped, unless --require-all is passed. Exits with status 1 if
codeowners-stats disagrees with GitHub. In GitHub Actions, it also writes a
table to the job summary.

    uv run compare/compare.py [--limit N] [--require-all]
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

from codeowners import CodeOwners

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent / "tests" / "github.txt"


def run(args: list[str], cwd: Path) -> list[str]:
    return subprocess.run(
        args, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.split("\n")


def rust() -> dict[str, list[str]]:
    lines = run(["cargo", "run", "-q", "--release", "--", str(FIXTURE)], HERE / "rust")
    rows = [line.split() for line in lines if line]
    return {
        "codeowners-stats": [row[0] for row in rows],
        "codeowners-rs": [row[1] for row in rows],
    }


def go() -> dict[str, list[str]]:
    lines = run(["go", "run", ".", str(FIXTURE)], HERE / "go")
    return {"hmarr/codeowners": [line for line in lines if line]}


def node() -> dict[str, list[str]]:
    run(["npm", "ci", "--no-audit", "--no-fund", "--silent"], HERE / "node")
    lines = run(["node", "main.js", str(FIXTURE)], HERE / "node")
    return {"snyk fork": [line for line in lines if line]}


def sbdchd(cases: list[tuple[str, str, str]]) -> dict[str, list[str]]:
    verdicts = []
    for line, path, _ in cases:
        owned = CodeOwners(line).matching_line(path)[1] is not None
        verdicts.append("1" if owned else "0")
    return {"sbdchd/codeowners": verdicts}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--limit", type=int, default=20, help="lines to show per library"
    )
    parser.add_argument(
        "--require-all", action="store_true", help="fail if a tool is missing"
    )
    args = parser.parse_args()

    cases = [
        tuple(line.split(" | "))
        for line in FIXTURE.read_text(encoding="utf-8").splitlines()
        if line and not line.startswith("#")
    ]
    results: dict[str, list[str]] = {}
    for tool, collect in [("cargo", rust), ("go", go), ("npm", node)]:
        if shutil.which(tool):
            results |= collect()
        elif args.require_all:
            sys.exit(f"{tool} isn't on PATH")
        else:
            print(f"skipping {collect.__name__}: {tool} isn't on PATH")
    results |= sbdchd(cases)

    print(f"{len(cases)} cases from {FIXTURE.relative_to(HERE.parent)}\n")
    summary = [
        f"Agreement with GitHub on {len(cases)} recorded cases:",
        "",
        "| Library | Agrees | Lines that differ |",
        "|---|---|---|",
    ]
    failed = False
    for name, verdicts in results.items():
        assert len(verdicts) == len(cases), (name, len(verdicts))
        # A rejected line owns nothing, like on GitHub.
        diffs = [
            (case, verdict)
            for case, verdict in zip(cases, verdicts)
            if (verdict == "1") != (case[2] == "1")
        ]
        print(f"{name}: agrees with GitHub on {len(cases) - len(diffs)}/{len(cases)}")
        # Group by CODEOWNERS line, so one pattern doesn't fill the output.
        by_line: dict[str, list[str]] = {}
        for (line, path, expected), verdict in diffs:
            by_line.setdefault(line, []).append(
                f"{path} (github {expected}, {verdict})"
            )
        for line, paths in list(by_line.items())[: args.limit]:
            more = f" and {len(paths) - 1} more" if len(paths) > 1 else ""
            print(f"  {line}: {paths[0]}{more}")
        if len(by_line) > args.limit:
            print(f"  ... and {len(by_line) - args.limit} more lines")
        print()
        summary.append(
            f"| {name} | {len(cases) - len(diffs)}/{len(cases)} | {len(by_line)} |"
        )
        if name == "codeowners-stats" and diffs:
            failed = True

    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a", encoding="utf-8") as f:
            f.write("\n".join(summary) + "\n")
    if failed:
        sys.exit("codeowners-stats disagrees with GitHub")


if __name__ == "__main__":
    main()
