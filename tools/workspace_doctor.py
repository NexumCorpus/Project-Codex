#!/usr/bin/env python3
"""Inspect Nexum Graph workspace health for Codex-driven development."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import spec_query
import verify_slice

REQUIRED_COMMANDS = {
    "git": ["git", "--version"],
    "cargo": ["cargo", "--version"],
    "clippy": ["cargo", "clippy", "--version"],
}

OPTIONAL_COMMANDS = {
    "rg": ["rg", "--version"],
}

EXPECTED_SKILLS = [
    "nexum-graph-sprint",
    "nexum-graph-maintainer",
]

SKIP_DIRS = {".git", ".nex", ".vs", "target", "__pycache__"}
SKIP_SUFFIXES = {
    ".db",
    ".docx",
    ".gif",
    ".ico",
    ".jpeg",
    ".jpg",
    ".lock",
    ".png",
    ".sqlite",
}
LEGACY_PATTERNS = ("Project Codex", "codex-", ".codex/")
LEGACY_ALLOWLIST = {
    ("tools/verify_slice.py", 'path.startswith("Project Codex/")'),
    ("tools/verify_slice.py", 'path = path[len("Project Codex/") :]'),
    ("tools/workspace_doctor.py", "LEGACY_PATTERNS ="),
    ("tools/workspace_doctor.py", "legacy Project Codex naming"),
    ("tools/workspace_doctor.py", 'and "Project Codex/" in line'),
}


@dataclass(frozen=True)
class CheckResult:
    """Single doctor check result."""

    status: str
    label: str
    detail: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Inspect Nexum Graph workspace health, tooling, and local skills."
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON instead of text.",
    )
    parser.add_argument(
        "--legacy-scan",
        action="store_true",
        help="Scan source files for legacy Project Codex naming outside .docx specs.",
    )
    parser.add_argument(
        "--legacy-limit",
        type=int,
        default=10,
        help="Maximum legacy naming hits to report. Defaults to 10.",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero when warnings are present.",
    )
    return parser.parse_args()


def actual_codex_home() -> Path:
    return Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))


def run_command(command: list[str]) -> tuple[bool, str]:
    executable = command[0]
    if not Path(executable).exists() and shutil.which(executable) is None:
        return False, "not found"

    try:
        completed = subprocess.run(
            command,
            cwd=verify_slice.repo_root(),
            check=True,
            capture_output=True,
            text=True,
        )
    except OSError:
        try:
            completed = subprocess.run(
                subprocess.list2cmdline(command),
                cwd=verify_slice.repo_root(),
                check=True,
                capture_output=True,
                text=True,
                shell=True,
            )
        except OSError as err:
            return False, str(err)
        except subprocess.CalledProcessError as err:
            stderr = err.stderr.strip() or err.stdout.strip() or f"exit {err.returncode}"
            return False, stderr
    except subprocess.CalledProcessError as err:
        stderr = err.stderr.strip() or err.stdout.strip() or f"exit {err.returncode}"
        return False, stderr

    output = completed.stdout.strip() or completed.stderr.strip() or "ok"
    return True, output.splitlines()[0]


def check_commands() -> list[CheckResult]:
    results = [CheckResult("ok", "python", sys.version.splitlines()[0])]
    for label, command in REQUIRED_COMMANDS.items():
        ok, detail = run_command(command)
        results.append(CheckResult("ok" if ok else "error", label, detail))
    for label, command in OPTIONAL_COMMANDS.items():
        ok, detail = run_command(command)
        results.append(CheckResult("ok" if ok else "warn", label, detail))
    return results


def check_documents() -> list[CheckResult]:
    results: list[CheckResult] = []
    for doc_key, filename in spec_query.DOCUMENTS.items():
        path = verify_slice.repo_root() / filename
        status = "ok" if path.exists() else "error"
        results.append(CheckResult(status, f"doc:{doc_key}", str(path)))
    return results


def check_skills() -> list[CheckResult]:
    results: list[CheckResult] = []
    skills_root = actual_codex_home() / "skills"
    for skill_name in EXPECTED_SKILLS:
        skill_path = skills_root / skill_name / "SKILL.md"
        status = "ok" if skill_path.exists() else "warn"
        results.append(CheckResult(status, f"skill:{skill_name}", str(skill_path)))
    return results


def check_workspace() -> tuple[list[CheckResult], dict]:
    workspace = verify_slice.load_workspace_info()
    changed_paths = verify_slice.unique_paths(verify_slice.git_status_paths())
    selected, non_crate_paths = verify_slice.infer_crates_from_paths(changed_paths, workspace)
    impacted = verify_slice.expand_dependents(selected, workspace, include_dependents=True)
    impacted_status = "ok"
    impacted_detail = ", ".join(impacted)
    if not impacted:
        impacted_detail = "(none)"
        if changed_paths and not selected and non_crate_paths:
            impacted_detail = "(none; non-crate changes only)"
        else:
            impacted_status = "warn"

    results = [
        CheckResult("ok", "workspace_root", str(workspace.workspace_root)),
        CheckResult("ok", "workspace_crates", ", ".join(workspace.crates_in_order)),
        CheckResult("ok", "dirty_paths", str(len(changed_paths))),
        CheckResult(
            impacted_status,
            "impacted_crates",
            impacted_detail,
        ),
    ]
    if non_crate_paths:
        status = "ok" if changed_paths and not selected else "warn"
        results.append(CheckResult(status, "non_crate_paths", ", ".join(non_crate_paths)))

    payload = {
        "changed_paths": changed_paths,
        "selected_crates": [crate for crate in workspace.crates_in_order if crate in selected],
        "impacted_crates": impacted,
        "non_crate_paths": non_crate_paths,
    }
    return results, payload


def scan_legacy_naming(limit: int) -> list[dict]:
    hits: list[dict] = []
    root = verify_slice.repo_root()
    for path in root.rglob("*"):
        if len(hits) >= limit:
            break
        if not path.is_file():
            continue
        if path.suffix.lower() in SKIP_SUFFIXES:
            continue
        relative = path.relative_to(root)
        if any(part in SKIP_DIRS for part in relative.parts):
            continue

        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue

        normalized_relative = str(relative).replace("\\", "/")
        for line_number, line in enumerate(text.splitlines(), start=1):
            if normalized_relative == "tools/workspace_doctor.py" and "tools/verify_slice.py" in line:
                continue
            for pattern in LEGACY_PATTERNS:
                if pattern in line:
                    if any(
                        normalized_relative == allowed_path and allowed_snippet in line
                        for allowed_path, allowed_snippet in LEGACY_ALLOWLIST
                    ):
                        break
                    hits.append(
                        {
                            "path": str(relative),
                            "line": line_number,
                            "pattern": pattern,
                            "text": line.strip(),
                        }
                    )
                    break
            if len(hits) >= limit:
                break
    return hits


def print_text(results: list[CheckResult], legacy_hits: list[dict], workspace_payload: dict) -> None:
    print("Workspace Doctor")
    print("================")
    for result in results:
        print(f"[{result.status.upper()}] {result.label}: {result.detail}")

    if workspace_payload["changed_paths"]:
        print("Changed paths:")
        for path in workspace_payload["changed_paths"]:
            print(f"  - {path}")

    if legacy_hits:
        print("Legacy naming hits:")
        for hit in legacy_hits:
            print(f"  - {hit['path']}:{hit['line']} [{hit['pattern']}] {hit['text']}")


def print_json(results: list[CheckResult], legacy_hits: list[dict], workspace_payload: dict) -> None:
    payload = {
        "results": [
            {"status": result.status, "label": result.label, "detail": result.detail}
            for result in results
        ],
        "workspace": workspace_payload,
        "legacy_hits": legacy_hits,
    }
    print(json.dumps(payload, indent=2))


def main() -> int:
    args = parse_args()

    results: list[CheckResult] = []
    results.extend(check_commands())
    results.extend(check_documents())
    results.extend(check_skills())
    workspace_results, workspace_payload = check_workspace()
    results.extend(workspace_results)

    legacy_hits = scan_legacy_naming(args.legacy_limit) if args.legacy_scan else []
    if args.legacy_scan:
        results.append(
            CheckResult(
                "warn" if legacy_hits else "ok",
                "legacy_naming",
                f"{len(legacy_hits)} hit(s)",
            )
        )

    if args.json:
        print_json(results, legacy_hits, workspace_payload)
    else:
        print_text(results, legacy_hits, workspace_payload)

    has_errors = any(result.status == "error" for result in results)
    has_warnings = any(result.status == "warn" for result in results)
    if has_errors:
        return 1
    if args.strict and has_warnings:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
