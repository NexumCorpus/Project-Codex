#!/usr/bin/env python3
"""Run dependency-aware verification for Nexum Graph workspace slices."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

WORKSPACE_ORDER = [
    "nex-core",
    "nex-parse",
    "nex-graph",
    "nex-coord",
    "nex-validate",
    "nex-eventlog",
    "nex-lsp",
    "nex-cli",
]

DEPENDENTS = {
    "nex-core": {
        "nex-parse",
        "nex-graph",
        "nex-coord",
        "nex-validate",
        "nex-eventlog",
        "nex-lsp",
        "nex-cli",
    },
    "nex-parse": {"nex-coord", "nex-lsp", "nex-cli"},
    "nex-graph": {"nex-coord", "nex-validate", "nex-lsp", "nex-cli"},
    "nex-coord": {"nex-cli"},
    "nex-validate": {"nex-cli"},
    "nex-eventlog": {"nex-lsp", "nex-cli"},
    "nex-lsp": set(),
    "nex-cli": set(),
}

SHARED_ROOT_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    "README.md",
    "Project_Codex_Final_Implementation_Spec.docx",
    "Project_Codex_Whitepaper_v1.docx",
    "Project_Codex_Whitepaper_v3.docx",
}


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run cargo test/clippy/fmt for the impacted Nexum Graph crates."
    )
    parser.add_argument(
        "--crate",
        dest="crates",
        action="append",
        default=[],
        help="Explicit workspace crate name, for example nex-parse.",
    )
    parser.add_argument(
        "--file",
        dest="files",
        action="append",
        default=[],
        help="Changed file path used to infer impacted crates.",
    )
    parser.add_argument(
        "--changed",
        action="store_true",
        help="Infer changed files from `git status --short`.",
    )
    parser.add_argument(
        "--full-sweep",
        action="store_true",
        help="Ignore inference and verify the full workspace crate list.",
    )
    parser.add_argument(
        "--steps",
        nargs="+",
        choices=["tests", "clippy", "fmt"],
        default=["tests", "clippy", "fmt"],
        help="Verification steps to run. Defaults to tests clippy fmt.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the computed crates and commands without running them.",
    )
    return parser.parse_args()


def normalize_path(raw_path: str) -> str:
    path = raw_path.strip().replace("\\", "/")
    if path.startswith("./"):
        path = path[2:]
    if path[:3].lower() == "e:/":
        path = path[3:]
        if path.startswith("Nexum-Graph/"):
            path = path[len("Nexum-Graph/") :]
    return path.lstrip("/")


def git_changed_paths() -> list[str]:
    result = subprocess.run(
        ["git", "status", "--short"],
        cwd=repo_root(),
        check=True,
        capture_output=True,
        text=True,
    )
    paths: list[str] = []
    for line in result.stdout.splitlines():
        if not line.strip():
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        paths.append(normalize_path(path))
    return paths


def infer_crates_from_paths(paths: list[str]) -> set[str]:
    crates: set[str] = set()
    for raw_path in paths:
        path = normalize_path(raw_path)
        if not path:
            continue
        parts = Path(path).parts
        if len(parts) >= 2 and parts[0] == "crates" and parts[1] in WORKSPACE_ORDER:
            crates.add(parts[1])
            continue
        if path in SHARED_ROOT_FILES or parts[:1] in [("tools",), ("prompts",)]:
            return set(WORKSPACE_ORDER)
    return crates


def expand_dependents(crates: set[str]) -> list[str]:
    expanded = set(crates)
    queue = list(crates)
    while queue:
        crate = queue.pop()
        for dependent in DEPENDENTS[crate]:
            if dependent not in expanded:
                expanded.add(dependent)
                queue.append(dependent)
    return [crate for crate in WORKSPACE_ORDER if crate in expanded]


def build_commands(crates: list[str], steps: list[str]) -> list[list[str]]:
    commands: list[list[str]] = []
    if "tests" in steps:
        commands.append(["cargo", "test", *sum((["-p", crate] for crate in crates), [])])
    if "clippy" in steps:
        commands.append(
            [
                "cargo",
                "clippy",
                *sum((["-p", crate] for crate in crates), []),
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ]
        )
    if "fmt" in steps:
        commands.append(
            ["cargo", "fmt", *sum((["-p", crate] for crate in crates), []), "--check"]
        )
    return commands


def main() -> int:
    args = parse_args()

    selected = set(args.crates)
    if args.full_sweep:
        selected = set(WORKSPACE_ORDER)
    else:
        if args.changed:
            args.files.extend(git_changed_paths())
        selected |= infer_crates_from_paths(args.files)

    if not selected:
        print("No crates selected. Use --crate, --file, --changed, or --full-sweep.", file=sys.stderr)
        return 2

    unknown = sorted(selected - set(WORKSPACE_ORDER))
    if unknown:
        print(f"Unknown crate(s): {', '.join(unknown)}", file=sys.stderr)
        return 2

    crates = expand_dependents(selected)
    commands = build_commands(crates, args.steps)

    print("Impacted crates:")
    for crate in crates:
        print(f"  - {crate}")

    print("Commands:")
    for command in commands:
        print(f"  {' '.join(command)}")

    if args.dry_run:
        return 0

    for command in commands:
        completed = subprocess.run(command, cwd=repo_root())
        if completed.returncode != 0:
            return completed.returncode

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
