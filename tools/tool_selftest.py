#!/usr/bin/env python3
"""Run regression and smoke checks for Nexum Graph repo tools and local skills."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

TOOLS = [
    "tools/spec_query.py",
    "tools/verify_slice.py",
    "tools/workspace_doctor.py",
    "tools/test_repo_tools.py",
    "tools/tool_selftest.py",
]

SKILLS = [
    Path.home() / ".codex" / "skills" / "nexum-graph-sprint",
    Path.home() / ".codex" / "skills" / "nexum-graph-maintainer",
]


@dataclass(frozen=True)
class StepResult:
    name: str
    status: str
    command: str
    detail: str


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run py_compile, unittest, workspace doctor, and optional skill validation."
    )
    parser.add_argument(
        "--skip-skills",
        action="store_true",
        help="Skip local skill validation.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON instead of text.",
    )
    return parser.parse_args()


def run_step(name: str, command: list[str]) -> StepResult:
    try:
        completed = subprocess.run(
            command,
            cwd=repo_root(),
            capture_output=True,
            text=True,
            check=True,
        )
    except subprocess.CalledProcessError as err:
        detail = err.stderr.strip() or err.stdout.strip() or f"exit {err.returncode}"
        return StepResult(name=name, status="error", command=" ".join(command), detail=detail)

    detail = completed.stdout.strip() or completed.stderr.strip() or "ok"
    return StepResult(name=name, status="ok", command=" ".join(command), detail=detail)


def build_steps(skip_skills: bool) -> list[tuple[str, list[str]]]:
    steps: list[tuple[str, list[str]]] = [
        ("py_compile", [sys.executable, "-m", "py_compile", *TOOLS]),
        (
            "unit_tests",
            [sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"],
        ),
        ("workspace_doctor", [sys.executable, "tools/workspace_doctor.py", "--json"]),
    ]

    if not skip_skills:
        validator = (
            Path.home()
            / ".codex"
            / "skills"
            / ".system"
            / "skill-creator"
            / "scripts"
            / "quick_validate.py"
        )
        for skill_dir in SKILLS:
            if skill_dir.exists():
                steps.append(
                    (
                        f"skill:{skill_dir.name}",
                        [sys.executable, str(validator), str(skill_dir)],
                    )
                )
            else:
                steps.append((f"skill:{skill_dir.name}", []))

    return steps


def print_text(results: list[StepResult]) -> None:
    print("Tool Selftest")
    print("=============")
    for result in results:
        print(f"[{result.status.upper()}] {result.name}: {result.command}")
        if result.detail:
            print(f"  {result.detail}")


def print_json(results: list[StepResult]) -> None:
    print(
        json.dumps(
            [
                {
                    "name": result.name,
                    "status": result.status,
                    "command": result.command,
                    "detail": result.detail,
                }
                for result in results
            ],
            indent=2,
        )
    )


def main() -> int:
    args = parse_args()
    results: list[StepResult] = []
    for name, command in build_steps(args.skip_skills):
        if not command:
            results.append(
                StepResult(
                    name=name,
                    status="warn",
                    command="(missing skill directory)",
                    detail="skill directory not found",
                )
            )
            continue
        results.append(run_step(name, command))

    if args.json:
        print_json(results)
    else:
        print_text(results)

    if any(result.status == "error" for result in results):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
