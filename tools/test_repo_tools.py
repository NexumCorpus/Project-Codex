from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from subprocess import CompletedProcess
from unittest import mock

TOOLS_DIR = Path(__file__).resolve().parent
if str(TOOLS_DIR) not in sys.path:
    sys.path.insert(0, str(TOOLS_DIR))

import spec_query
import verify_slice
import workspace_doctor


class VerifySliceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workspace = verify_slice.WorkspaceInfo(
            workspace_root=Path("E:/Project Codex"),
            crates_in_order=["nex-core", "nex-parse", "nex-graph", "nex-cli"],
            crate_dirs={
                "nex-core": "crates/nex-core",
                "nex-parse": "crates/nex-parse",
                "nex-graph": "crates/nex-graph",
                "nex-cli": "crates/nex-cli",
            },
            dependents={
                "nex-core": {"nex-parse", "nex-graph", "nex-cli"},
                "nex-parse": {"nex-cli"},
                "nex-graph": {"nex-cli"},
                "nex-cli": set(),
            },
        )

    def test_normalize_path_handles_windows_and_legacy_prefixes(self) -> None:
        self.assertEqual(
            verify_slice.normalize_path(r"E:\Project Codex\crates\nex-parse\src\lib.rs"),
            "crates/nex-parse/src/lib.rs",
        )
        self.assertEqual(
            verify_slice.normalize_path("E:/Nexum-Graph/crates/nex-cli/src/main.rs"),
            "crates/nex-cli/src/main.rs",
        )
        self.assertEqual(
            verify_slice.normalize_path("E:/Project Codex/tools/spec_query.py"),
            "tools/spec_query.py",
        )

    def test_unique_paths_filters_python_cache_noise(self) -> None:
        result = verify_slice.unique_paths(
            [
                "tools/spec_query.py",
                "tools/spec_query.py",
                "tools/__pycache__/spec_query.cpython-313.pyc",
                "tools/__pycache__/",
                "./tools/verify_slice.py",
            ]
        )
        self.assertEqual(result, ["tools/spec_query.py", "tools/verify_slice.py"])

    def test_infer_crates_from_paths_distinguishes_non_crate_files(self) -> None:
        crates, non_crate_paths = verify_slice.infer_crates_from_paths(
            ["crates/nex-parse/src/lib.rs", "tools/spec_query.py"],
            self.workspace,
        )
        self.assertEqual(crates, {"nex-parse"})
        self.assertEqual(non_crate_paths, ["tools/spec_query.py"])

    def test_root_manifest_triggers_full_sweep(self) -> None:
        crates, non_crate_paths = verify_slice.infer_crates_from_paths(["Cargo.toml"], self.workspace)
        self.assertEqual(crates, set(self.workspace.crates_in_order))
        self.assertEqual(non_crate_paths, [])

    def test_expand_dependents_respects_flag(self) -> None:
        self.assertEqual(
            verify_slice.expand_dependents({"nex-parse"}, self.workspace, include_dependents=False),
            ["nex-parse"],
        )
        self.assertEqual(
            verify_slice.expand_dependents({"nex-parse"}, self.workspace, include_dependents=True),
            ["nex-parse", "nex-cli"],
        )

    def test_build_commands_emits_requested_steps(self) -> None:
        commands = verify_slice.build_commands(["nex-parse", "nex-cli"], ["tests", "fmt"])
        self.assertEqual(
            commands,
            [
                ["cargo", "test", "-p", "nex-parse", "-p", "nex-cli"],
                ["cargo", "fmt", "-p", "nex-parse", "-p", "nex-cli", "--check"],
            ],
        )


class SpecQueryTests(unittest.TestCase):
    def test_matches_line_supports_all_modes(self) -> None:
        line = "Intent, locking, CRDT coordination"
        self.assertTrue(spec_query.matches_line(line, ["intent", "crdt"], "all"))
        self.assertTrue(spec_query.matches_line(line, ["rollback", "crdt"], "any"))
        self.assertTrue(spec_query.matches_line(line, ["CRDT", "coordination"], "phrase"))
        self.assertTrue(spec_query.matches_line(line, [r"lock\w+"], "regex"))

    def test_search_lines_returns_context_window(self) -> None:
        matches = spec_query.search_lines(
            ["alpha", "beta gamma", "delta"],
            ["beta"],
            context=1,
            mode="all",
        )
        self.assertEqual(
            matches,
            [
                {
                    "line_number": 2,
                    "line": "beta gamma",
                    "window": ["1: alpha", "2: beta gamma", "3: delta"],
                }
            ],
        )

    def test_cache_round_trip_uses_mtime_guard(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            source_path = temp_path / "spec.docx"
            source_path.write_bytes(b"placeholder")

            with mock.patch.object(spec_query, "cache_dir", return_value=temp_path):
                spec_query.write_cache("spec", source_path, ["line one", "line two"])
                cached = spec_query.read_cached_lines("spec", source_path)
                self.assertEqual(cached, ["line one", "line two"])

                source_path.write_bytes(b"changed")
                self.assertIsNone(spec_query.read_cached_lines("spec", source_path))


class WorkspaceDoctorTests(unittest.TestCase):
    def test_actual_codex_home_prefers_environment(self) -> None:
        with mock.patch.dict(workspace_doctor.os.environ, {"CODEX_HOME": "C:/tmp/codex-home"}):
            self.assertEqual(
                workspace_doctor.actual_codex_home(),
                Path("C:/tmp/codex-home"),
            )

    def test_run_command_falls_back_to_shell_when_windows_launch_fails(self) -> None:
        first_error = OSError("Access is denied.")
        second_result = CompletedProcess(["git", "--version"], 0, stdout="git version 2.50.1", stderr="")
        with (
            mock.patch.object(workspace_doctor.shutil, "which", return_value="C:/Program Files/Git/cmd/git.exe"),
            mock.patch.object(
                workspace_doctor.subprocess,
                "run",
                side_effect=[first_error, second_result],
            ) as run_mock,
        ):
            ok, detail = workspace_doctor.run_command(["git", "--version"])

        self.assertTrue(ok)
        self.assertEqual(detail, "git version 2.50.1")
        self.assertEqual(run_mock.call_count, 2)

    def test_check_workspace_treats_tool_only_changes_as_ok(self) -> None:
        workspace = verify_slice.WorkspaceInfo(
            workspace_root=Path("E:/Project Codex"),
            crates_in_order=["nex-core", "nex-cli"],
            crate_dirs={"nex-core": "crates/nex-core", "nex-cli": "crates/nex-cli"},
            dependents={"nex-core": {"nex-cli"}, "nex-cli": set()},
        )
        with (
            mock.patch.object(verify_slice, "load_workspace_info", return_value=workspace),
            mock.patch.object(verify_slice, "git_status_paths", return_value=["tools/spec_query.py"]),
        ):
            results, payload = workspace_doctor.check_workspace()

        impacted = next(result for result in results if result.label == "impacted_crates")
        non_crate = next(result for result in results if result.label == "non_crate_paths")
        self.assertEqual(impacted.status, "ok")
        self.assertEqual(impacted.detail, "(none; non-crate changes only)")
        self.assertEqual(non_crate.status, "ok")
        self.assertEqual(payload["non_crate_paths"], ["tools/spec_query.py"])

    def test_scan_legacy_naming_ignores_allowlisted_and_skipped_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            (root / "tools").mkdir()
            (root / ".vs").mkdir()
            (root / "tools" / "workspace_doctor.py").write_text(
                'LEGACY_PATTERNS = ("Project Codex",)\n'
                '("tools/verify_slice.py", \'path.startswith("Project Codex/")\')\n',
                encoding="utf-8",
            )
            (root / "src.txt").write_text("Project Codex rename target\n", encoding="utf-8")
            (root / ".vs" / "cache.sqlite").write_text("Project Codex", encoding="utf-8")

            with mock.patch.object(verify_slice, "repo_root", return_value=root):
                hits = workspace_doctor.scan_legacy_naming(limit=10)

        self.assertEqual(
            hits,
            [
                {
                    "path": "src.txt",
                    "line": 1,
                    "pattern": "Project Codex",
                    "text": "Project Codex rename target",
                }
            ],
        )


if __name__ == "__main__":
    unittest.main()
