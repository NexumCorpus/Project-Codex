#!/usr/bin/env python3
"""Run dependency-aware verification for Nexum Graph workspace slices."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT_FULL_SWEEP_FILES = {"Cargo.toml", "Cargo.lock"}
IGNORED_PATH_PARTS = {"__pycache__"}
IGNORED_SUFFIXES = {".pyc"}


@dataclass(frozen=True)
class WorkspaceInfo:
    """Derived Cargo workspace metadata used for slice verification."""

    workspace_root: Path
    crates_in_order: list[str]
    crate_dirs: dict[str, str]
    dependents: dict[str, set[str]]


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
        "--since",
        help="Infer changed files from `git diff --name-only <rev>...HEAD`.",
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
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON instead of text.",
    )
    parser.add_argument(
        "--list-crates",
        action="store_true",
        help="Print the discovered workspace crates and exit.",
    )
    parser.add_argument(
        "--no-dependents",
        action="store_true",
        help="Skip transitive dependent expansion and verify only the selected crates.",
    )
    parser.add_argument(
        "--strict-empty",
        action="store_true",
        help="Exit with code 2 when no Rust crates are impacted.",
    )
    return parser.parse_args()


def normalize_path(raw_path: str) -> str:
    path = raw_path.strip().replace("\\", "/")
    if not path:
        return ""
    if path.startswith("./"):
        path = path[2:]
    if len(path) >= 3 and path[1:3] == ":/":
        path = path[3:]
        if path.startswith("Nexum-Graph/"):
            path = path[len("Nexum-Graph/") :]
        elif path.startswith("Project Codex/"):
            path = path[len("Project Codex/") :]
    return path.lstrip("/")


def cargo_metadata() -> dict:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=repo_root(),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def load_workspace_info() -> WorkspaceInfo:
    metadata = cargo_metadata()
    root = Path(metadata["workspace_root"]).resolve()
    packages_by_id = {package["id"]: package for package in metadata["packages"]}
    member_packages = [packages_by_id[member_id] for member_id in metadata["workspace_members"]]
    member_names = [package["name"] for package in member_packages]
    member_name_set = set(member_names)

    crate_dirs: dict[str, str] = {}
    dependents = {name: set() for name in member_names}
    for package in member_packages:
        crate_dirs[package["name"]] = normalize_path(
            str(Path(package["manifest_path"]).resolve().parent.relative_to(root))
        )

    for package in member_packages:
        crate_name = package["name"]
        for dependency in package["dependencies"]:
            dependency_name = dependency["name"]
            if dependency_name in member_name_set:
                dependents[dependency_name].add(crate_name)

    return WorkspaceInfo(
        workspace_root=root,
        crates_in_order=member_names,
        crate_dirs=crate_dirs,
        dependents=dependents,
    )


def git_status_paths() -> list[str]:
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


def git_diff_paths(revision: str) -> list[str]:
    result = subprocess.run(
        ["git", "diff", "--name-only", f"{revision}...HEAD"],
        cwd=repo_root(),
        check=True,
        capture_output=True,
        text=True,
    )
    return [normalize_path(line) for line in result.stdout.splitlines() if line.strip()]


def unique_paths(paths: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for path in paths:
        normalized = normalize_path(path)
        if not normalized or normalized in seen:
            continue
        if any(part in IGNORED_PATH_PARTS for part in Path(normalized).parts):
            continue
        if Path(normalized).suffix.lower() in IGNORED_SUFFIXES:
            continue
        seen.add(normalized)
        ordered.append(normalized)
    return ordered


def infer_crates_from_paths(paths: list[str], workspace: WorkspaceInfo) -> tuple[set[str], list[str]]:
    crates: set[str] = set()
    non_crate_paths: list[str] = []

    for path in paths:
        if path in ROOT_FULL_SWEEP_FILES:
            return set(workspace.crates_in_order), []

        matched = False
        for crate in workspace.crates_in_order:
            crate_dir = workspace.crate_dirs[crate]
            if path == crate_dir or path.startswith(f"{crate_dir}/"):
                crates.add(crate)
                matched = True
                break

        if not matched:
            non_crate_paths.append(path)

    return crates, non_crate_paths


def expand_dependents(selected: set[str], workspace: WorkspaceInfo, include_dependents: bool) -> list[str]:
    if not include_dependents:
        return [crate for crate in workspace.crates_in_order if crate in selected]

    expanded = set(selected)
    queue = list(selected)
    while queue:
        crate = queue.pop()
        for dependent in workspace.dependents.get(crate, set()):
            if dependent not in expanded:
                expanded.add(dependent)
                queue.append(dependent)

    return [crate for crate in workspace.crates_in_order if crate in expanded]


def build_commands(crates: list[str], steps: list[str]) -> list[list[str]]:
    commands: list[list[str]] = []
    if not crates:
        return commands
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
        commands.append(["cargo", "fmt", *sum((["-p", crate] for crate in crates), []), "--check"])
    return commands


def print_text_summary(crates: list[str], commands: list[list[str]], non_crate_paths: list[str]) -> None:
    if crates:
        print("Impacted crates:")
        for crate in crates:
            print(f"  - {crate}")
    else:
        print("No Rust crates impacted.")

    if non_crate_paths:
        print("Non-crate paths:")
        for path in non_crate_paths:
            print(f"  - {path}")

    if commands:
        print("Commands:")
        for command in commands:
            print(f"  {' '.join(command)}")


def print_json_summary(
    selected_crates: list[str],
    impacted_crates: list[str],
    changed_paths: list[str],
    non_crate_paths: list[str],
    commands: list[list[str]],
) -> None:
    payload = {
        "selected_crates": selected_crates,
        "impacted_crates": impacted_crates,
        "changed_paths": changed_paths,
        "non_crate_paths": non_crate_paths,
        "commands": commands,
    }
    print(json.dumps(payload, indent=2))


def main() -> int:
    args = parse_args()
    workspace = load_workspace_info()

    if args.list_crates:
        for crate in workspace.crates_in_order:
            print(crate)
        return 0

    changed_paths: list[str] = list(args.files)
    if args.changed:
        changed_paths.extend(git_status_paths())
    if args.since:
        changed_paths.extend(git_diff_paths(args.since))
    changed_paths = unique_paths(changed_paths)

    selected = set(args.crates)
    inferred_non_crate_paths: list[str] = []
    if args.full_sweep:
        selected = set(workspace.crates_in_order)
    else:
        inferred, inferred_non_crate_paths = infer_crates_from_paths(changed_paths, workspace)
        selected |= inferred

    unknown = sorted(selected - set(workspace.crates_in_order))
    if unknown:
        print(f"Unknown crate(s): {', '.join(unknown)}", file=sys.stderr)
        return 2

    impacted = expand_dependents(selected, workspace, include_dependents=not args.no_dependents)
    commands = build_commands(impacted, args.steps)

    if args.json:
        print_json_summary(
            selected_crates=[crate for crate in workspace.crates_in_order if crate in selected],
            impacted_crates=impacted,
            changed_paths=changed_paths,
            non_crate_paths=inferred_non_crate_paths,
            commands=commands,
        )
    else:
        print_text_summary(impacted, commands, inferred_non_crate_paths)

    if not impacted:
        return 2 if args.strict_empty else 0

    if args.dry_run:
        return 0

    for command in commands:
        completed = subprocess.run(command, cwd=repo_root())
        if completed.returncode != 0:
            return completed.returncode

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
