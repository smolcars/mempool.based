#!/usr/bin/env python3
"""Checks the dependencies we ship against the advisory databases (docs/CI.md#dependency-audit).

Cargo.lock goes to cargo-audit (RustSec), web/bun.lock to `bun audit` (the npm registry's).
Advisories accepted in ignore.toml beside this file are skipped. Exits 0 when nothing applies,
1 with a Markdown report on stdout when an advisory does, 2 when a tool or ignore.toml fails.

Run from the repository root, with cargo-audit and bun on PATH:
  python3 -I dev/audit/audit.py
CARGO_LOCK and WEB_DIR point it at other lockfiles, to test it on a temporary copy.
"""

import datetime
import json
import os
import re
import subprocess
import sys
import tomllib
from collections import deque

IGNORE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ignore.toml")
CARGO_LOCK = os.environ.get("CARGO_LOCK", "Cargo.lock")
WEB_DIR = os.environ.get("WEB_DIR", "web")


def fail(message):
    print(f"audit: {message}", file=sys.stderr)
    sys.exit(2)


def ignored():
    """The accepted advisory ids. An entry without its reason and review date is an error."""
    with open(IGNORE_FILE, "rb") as f:
        entries = tomllib.load(f).get("ignore", [])
    for e in entries:
        reason = e.get("reason")
        if not (
            isinstance(e.get("id"), str)
            and re.fullmatch(r"(RUSTSEC-\d{4}-\d{4}|GHSA(-[0-9a-z]{4}){3})", e["id"])
            and isinstance(reason, str)
            and reason.strip()
            and isinstance(e.get("reviewed"), datetime.date)
        ):
            fail(f"ignore.toml: each [[ignore]] needs an advisory id, a reason and a reviewed date: {e}")
    return {e["id"] for e in entries}


def run(cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env={**os.environ, "NO_COLOR": "1"})


def run_json(cmd, cwd=None):
    p = run(cmd, cwd)
    try:
        return json.loads(p.stdout)
    except json.JSONDecodeError:
        fail(f"{' '.join(cmd[:2])} exited {p.returncode}: {p.stderr.strip()}")


def path_to_workspace(lock_path, name, version):
    """The shortest chain from a locked package up to one of our crates, as [package, ..., ours]."""
    with open(lock_path, "rb") as f:
        packages = tomllib.load(f)["package"]
    versions = {}
    for p in packages:
        versions.setdefault(p["name"], []).append(p["version"])
    dependents = {}
    for p in packages:
        for dep in p.get("dependencies", []):
            # "name", or "name version" when the lockfile holds more than one version of it.
            parts = dep.split(" ")
            dep_version = parts[1] if len(parts) > 1 else versions[parts[0]][0]
            dependents.setdefault((parts[0], dep_version), []).append(p)
    start = (name, version)
    queue = deque([[start]])
    seen = {start}
    ours = {(p["name"], p["version"]) for p in packages if "source" not in p}
    while queue:
        chain = queue.popleft()
        if chain[-1] in ours:
            return chain
        for p in dependents.get(chain[-1], []):
            key = (p["name"], p["version"])
            if key not in seen:
                seen.add(key)
                queue.append(chain + [key])
    return [start]


def cell(text):
    return str(text).replace("|", "\\|").replace("\n", " ")


def rust(ids):
    cmd = ["cargo-audit", "audit", "--json", "--deny", "warnings", "--file", CARGO_LOCK]
    for i in sorted(ids):
        if i.startswith("RUSTSEC-"):
            cmd += ["--ignore", i]
    report = run_json(cmd)
    found = report["vulnerabilities"]["list"] + [w for ws in report["warnings"].values() for w in ws]
    if not found:
        return None
    rows = [
        "| Advisory | Package | How we depend on it | Fixed in |",
        "|---|---|---|---|",
    ]
    for f in found:
        pkg = f["package"]
        adv = f.get("advisory")
        kind = f.get("kind")
        if adv:
            label = f"[{adv['id']}](https://rustsec.org/advisories/{adv['id']}): {cell(adv['title'])}"
            if kind:
                label += f" ({kind})"
        else:
            label = f"{kind}: this release was withdrawn from crates.io"
        chain = path_to_workspace(CARGO_LOCK, pkg["name"], pkg["version"])
        if len(chain) == 1:
            how = "not reachable from our crates"
        elif len(chain) == 2:
            how = f"direct, from `{chain[1][0]}`"
        else:
            how = "through " + " ← ".join(f"`{n} {v}`" for n, v in chain[1:])
        patched = (f.get("versions") or {}).get("patched") or []
        fixed = " or ".join(f"`{cell(v)}`" for v in patched) or "no fixed release"
        rows.append(f"| {label} | `{pkg['name']} {pkg['version']}` | {how} | {fixed} |")
    return "\n".join(rows)


def javascript(ids):
    flags = []
    for i in sorted(ids):
        if i.startswith("GHSA-"):
            flags += ["--ignore", i]
    # bun audit --json leaves out nothing --ignore names, so the JSON is filtered here.
    report = run_json(["bun", "audit", "--json"], WEB_DIR)
    found = [
        a
        for advisories in report.values()
        for a in advisories
        if a.get("url", "").rsplit("/", 1)[-1] not in ids
    ]
    if not found:
        return None
    # The text output names the installed version and the chain that pulls each package in.
    text = run(["bun", "audit", *flags], WEB_DIR).stdout.strip()
    return f"````\n{text}\n````\nA version range ending `<X` is fixed in `X`."


def main():
    ids = ignored()
    sections = []
    for title, section in (
        ("Rust: `Cargo.lock` against RustSec", rust(ids)),
        ("JavaScript: `web/bun.lock` against `bun audit`", javascript(ids)),
    ):
        if section:
            sections.append(f"## {title}\n\n{section}")
    if not sections:
        return 0
    print(
        "The dependency audit found advisories against what we ship. Fix each one, or accept it on "
        "purpose in `dev/audit/ignore.toml` with its reason ([docs/CI.md](../blob/master/docs/CI.md#dependency-audit)).\n"
    )
    print("\n\n".join(sections))
    return 1


if __name__ == "__main__":
    sys.exit(main())
