#!/usr/bin/env python3
"""Guard: the vulnerable libcrux crypto crates must stay out of the build graph.

`.cargo/audit.toml` ignores several libcrux advisories on the basis that the
crates carrying them are pulled into `Cargo.lock` (via `hpke-rs`'s optional,
*disabled* `libcrux` backend) but are **not** compiled or linked into the
Audium binary. DAVE/MLS uses the RustCrypto HPKE backend
(`hpke-rs-rust-crypto`), not `hpke-rs-libcrux`.

If anyone ever enables `hpke-rs`'s `libcrux` feature, or a dependency starts
pulling `hpke-rs-libcrux` unconditionally, those advisories become reachable and
the ignores would silently hide a real vulnerability. This check fails in that
case so the exceptions can be re-reviewed.

Requires `cargo` and a resolvable workspace. Exit 0 when the graph is as
expected, 1 otherwise.
"""

from __future__ import annotations

import subprocess
import sys

# Crates whose advisories are ignored *because they are not compiled*.
MUST_BE_ABSENT = [
    "hpke-rs-libcrux",
    "libcrux-kem",
    "libcrux-aesgcm",
    "libcrux-aead",
    "libcrux-chacha20poly1305",
]

# The HPKE backend that must remain selected.
EXPECTED_BACKEND = "hpke-rs-rust-crypto"


def run(args: list[str]) -> str:
    return subprocess.run(args, capture_output=True, text=True, check=True).stdout


def main() -> int:
    problems: list[str] = []

    tree = run(["cargo", "tree", "-e", "normal", "--prefix", "none"])

    for crate in MUST_BE_ABSENT:
        if any(line.split()[0] == crate for line in tree.splitlines() if line.strip()):
            problems.append(
                f"{crate} is now in the compiled dependency graph but its "
                f"advisory is still ignored in .cargo/audit.toml — re-review."
            )

    if EXPECTED_BACKEND not in tree:
        problems.append(
            f"expected HPKE backend {EXPECTED_BACKEND} not found in the graph; "
            f"the crypto backend selection changed — re-review the audit ignores."
        )

    # hpke-rs must not have its `libcrux` feature enabled.
    feats = run(["cargo", "tree", "-f", "{p} feats={f}", "-p", "hpke-rs"])
    hpke_line = next(
        (l for l in feats.splitlines() if l.startswith("hpke-rs v")), ""
    )
    enabled = {
        f.strip()
        for f in hpke_line.split("feats=")[-1].split(",")
        if f.strip()
    } if "feats=" in hpke_line else set()
    if "libcrux" in enabled:
        problems.append(
            "hpke-rs `libcrux` feature is ENABLED — hpke-rs-libcrux is now "
            "reachable; the libcrux audit ignores are no longer justified."
        )

    if problems:
        for p in problems:
            print(f"FAIL: {p}")
        return 1
    print(
        "OK: vulnerable libcrux crates absent from the build graph; "
        f"hpke-rs `libcrux` feature off; backend {EXPECTED_BACKEND} present"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
