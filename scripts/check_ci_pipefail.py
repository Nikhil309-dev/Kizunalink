#!/usr/bin/env python3
"""Guard: every `run:` block that pipes output through `tee` must set pipefail.

Without `set -o pipefail`, a bash pipeline's exit status is the *last* command's
(`tee`), which is always 0 — so a failing tool inside the pipe is silently
reported as a passing CI step. GitHub's implicit `bash -e` does not set
`pipefail`. This check fails if any such pipeline in `.github/workflows/*.yml`
does not enable it in the same block.

Exit code 0 when clean, 1 when a violation is found.
"""

from __future__ import annotations

import pathlib
import re
import sys

WORKFLOWS = pathlib.Path(__file__).resolve().parent.parent / ".github" / "workflows"
PIPE_RE = re.compile(r"\|\s*tee\b")


def indentation(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def check_file(path: pathlib.Path) -> list[str]:
    lines = path.read_text().splitlines()
    problems: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if stripped.startswith("run:") and ("|" in stripped or stripped == "run:"):
            base = indentation(line)
            body: list[tuple[int, str]] = []
            j = i + 1
            while j < len(lines):
                nxt = lines[j]
                if nxt.strip() == "":
                    body.append((j + 1, nxt))
                    j += 1
                    continue
                if indentation(nxt) <= base:
                    break
                body.append((j + 1, nxt))
                j += 1
            has_pipefail = any("set -o pipefail" in b for _, b in body)
            for lineno, b in body:
                if b.lstrip().startswith("#"):
                    continue
                if PIPE_RE.search(b) and not has_pipefail:
                    problems.append(
                        f"{path}:{lineno}: `| tee` pipeline without `set -o pipefail` "
                        f"in the same run block: {b.strip()}"
                    )
            i = j
        else:
            i += 1
    return problems


def main() -> int:
    problems: list[str] = []
    for path in sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml")):
        problems.extend(check_file(path))
    if problems:
        for p in problems:
            print(f"FAIL: {p}")
        return 1
    print("OK: all `| tee` pipelines set `set -o pipefail`")
    return 0


if __name__ == "__main__":
    sys.exit(main())
