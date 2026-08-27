from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.delta_workflow import (
    DEVELOPMENT_BRANCH,
    DEVELOPMENT_REMOTE_REF,
    DeltaWorkflowError,
)
from scripts.local_windows_installer import (
    DEFAULT_PATHS,
    InstallerIdentity,
    InstallerPaths,
    LocalInstallerError,
    OUTPUT_PATH,
    TARGET_ROOT,
    _bundle_manifest,
    _candidate_build_id,
    _candidate_paths,
    _cargo_build_arguments,
    _cargo_test_arguments,
    _dynamic_msvc_runtime_imports,
    _git_arguments,
    _hashes,
    _isolated_candidate_paths,
    _just_test_arguments,
    _require_pushed_development_source,
    _require_one_focused_test,
    _require_one_nextest_test,
    _source_branch,
    candidate,
    parse_identity,
)


BUILD_ID = "0123456789ab.cdef01234567"


class LocalWindowsInstallerTests(unittest.TestCase):
    def test_identity_parser_accepts_only_local_candidate_contract(self) -> None:
        self.assertEqual(
            parse_identity(
                f"herdr-win local (Herdr 0.8.0, build {BUILD_ID})", BUILD_ID
            ),
            InstallerIdentity(BUILD_ID, "0.8.0"),
        )
        with self.assertRaisesRegex(LocalInstallerError, "not an exact local build"):
            parse_identity("herdr-win 2026.08.15.1 (Herdr 0.8.0)", BUILD_ID)
        with self.assertRaisesRegex(LocalInstallerError, "different local build"):
            parse_identity(
                "herdr-win local (Herdr 0.8.0, build aaaaaaaaaaaa.bbbbbbbbbbbb)",
                BUILD_ID,
            )

    def test_bundle_manifest_detects_input_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bundle = Path(temporary)
            stage = bundle / "stage"
            stage.mkdir()
            (stage / "herdr.exe").write_bytes(b"runtime")
            (bundle / "herdr-launcher.exe").write_bytes(b"launcher")
            (bundle / "herdr-installer-helper.exe").write_bytes(b"helper")
            files = _hashes(bundle)
            (bundle / "bundle.json").write_text(
                json.dumps(
                    {
                        "schema": 1,
                        "build_id": BUILD_ID,
                        "base_version": "0.8.0",
                        "files": files,
                    }
                ),
                encoding="utf-8",
            )

            identity, expected = _bundle_manifest(bundle)
            self.assertEqual(identity, InstallerIdentity(BUILD_ID, "0.8.0"))
            self.assertEqual(_hashes(bundle), expected)

            (stage / "herdr.exe").write_bytes(b"changed")
            self.assertNotEqual(_hashes(bundle), expected)
            self.assertEqual(
                expected["stage/herdr.exe"], hashlib.sha256(b"runtime").hexdigest()
            )

    def test_default_output_is_one_short_replaceable_candidate_path(self) -> None:
        self.assertEqual(OUTPUT_PATH.name, "herdr-win_local_candidate_setup.exe")
        self.assertEqual(DEFAULT_PATHS.output_path, OUTPUT_PATH)

    def test_isolated_candidate_paths_share_one_build_scoped_root(self) -> None:
        paths = _isolated_candidate_paths(BUILD_ID)
        expected_root = TARGET_ROOT / "isolated" / BUILD_ID

        self.assertEqual(paths.target_root, expected_root)
        self.assertEqual(paths.input_root, expected_root / "installer-inputs")
        self.assertEqual(
            paths.output_path,
            expected_root / "release" / "herdr-win_local_candidate_setup.exe",
        )
        self.assertEqual(paths.nsis_cache, expected_root / "tools" / "nsis-3.12")

    def test_isolated_candidate_paths_reject_unbounded_names(self) -> None:
        with self.assertRaisesRegex(LocalInstallerError, "invalid candidate build ID"):
            _isolated_candidate_paths("../other-session")

    def test_individual_candidate_defaults_to_isolated_outputs(self) -> None:
        self.assertEqual(
            _candidate_paths(
                "agent/delta-one-fix", BUILD_ID, isolated=False
            ),
            _isolated_candidate_paths(BUILD_ID),
        )

    def test_development_branch_defaults_to_canonical_output(self) -> None:
        self.assertEqual(
            _candidate_paths(
                DEVELOPMENT_BRANCH,
                BUILD_ID,
                isolated=False,
            ),
            DEFAULT_PATHS,
        )

    def test_development_branch_can_force_isolated_outputs(self) -> None:
        self.assertEqual(
            _candidate_paths(
                DEVELOPMENT_BRANCH,
                BUILD_ID,
                isolated=True,
            ),
            _isolated_candidate_paths(BUILD_ID),
        )

    def test_development_installer_requires_clean_pushed_source(self) -> None:
        source = Path("C:/development")
        with (
            patch("scripts.local_windows_installer._git") as git,
            patch("scripts.local_windows_installer._run") as run,
            patch(
                "scripts.local_windows_installer.unintegrated_topic_worktrees",
                return_value=(),
            ),
        ):
            git.side_effect = ["", "a" * 40, "a" * 40]

            _require_pushed_development_source(
                source, DEVELOPMENT_BRANCH, isolated=False
            )

            self.assertIn(DEVELOPMENT_REMOTE_REF, run.call_args.args[1][-1])

        with (
            patch("scripts.local_windows_installer._git") as git,
            patch("scripts.local_windows_installer._run"),
            self.assertRaisesRegex(LocalInstallerError, "must equal"),
        ):
            git.side_effect = ["", "a" * 40, "b" * 40]
            _require_pushed_development_source(
                source, DEVELOPMENT_BRANCH, isolated=False
            )

    def test_source_branch_accepts_the_cumulative_development_branch(self) -> None:
        with patch(
            "scripts.local_windows_installer._git", return_value=DEVELOPMENT_BRANCH
        ):
            self.assertEqual(_source_branch(Path("C:/development")), DEVELOPMENT_BRANCH)

    def test_legacy_combined_branch_is_not_canonical(self) -> None:
        self.assertEqual(
            _candidate_paths(
                "agent/delta-combined-acceptance-20260823",
                BUILD_ID,
                isolated=False,
            ),
            _isolated_candidate_paths(BUILD_ID),
        )

    def test_candidate_identity_tracks_the_exact_source_snapshot(self) -> None:
        base = "a" * 40
        source = "b" * 40
        clean = _candidate_build_id(base, source, "", [])

        self.assertRegex(clean, r"^a{12}\.[0-9a-f]{12}$")
        self.assertNotEqual(clean, _candidate_build_id(base, source, "diff", []))
        self.assertNotEqual(
            clean,
            _candidate_build_id(base, source, "", [("src/new.rs", b"content")]),
        )

    def test_build_only_candidate_reuses_exact_bundle_before_cargo(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            paths = InstallerPaths(
                target_root=root / "target",
                input_root=root / "inputs",
                output_path=root / "release" / OUTPUT_PATH.name,
                nsis_cache=root / "tools" / "nsis",
            )
            bundle = paths.input_root / BUILD_ID
            bundle.mkdir(parents=True)
            options = argparse.Namespace(
                source_worktree=source,
                cargo_target_dir=root / "cargo-target",
                test_filter=None,
                release_test_filter=None,
                isolated=False,
            )

            with (
                patch(
                    "scripts.local_windows_installer._source_root",
                    return_value=source,
                ),
                patch(
                    "scripts.local_windows_installer._source_branch",
                    return_value=DEVELOPMENT_BRANCH,
                ),
                patch("scripts.local_windows_installer._require_pushed_development_source"),
                patch(
                    "scripts.local_windows_installer.validate_changed_integration_asset_versions",
                    return_value=(),
                ) as version_gate,
                patch(
                    "scripts.local_windows_installer._source_build_identity",
                    return_value=(BUILD_ID, "a" * 40),
                ),
                patch(
                    "scripts.local_windows_installer._candidate_paths",
                    return_value=paths,
                ),
                patch("scripts.local_windows_installer._directory") as directory,
                patch("scripts.local_windows_installer.build") as package,
            ):
                candidate(options)

            directory.assert_not_called()
            version_gate.assert_called_once_with(source)
            package.assert_called_once()
            self.assertEqual(package.call_args.args[0].input_bundle, bundle)
            self.assertEqual(package.call_args.kwargs["paths"], paths)
            self.assertTrue(package.call_args.kwargs["run_interactive_probe"])

    def test_candidate_validates_integrations_before_bundle_reuse(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            options = argparse.Namespace(
                source_worktree=source,
                cargo_target_dir=root / "cargo-target",
                test_filter=None,
                release_test_filter=None,
                isolated=False,
            )
            with (
                patch(
                    "scripts.local_windows_installer._source_root",
                    return_value=source,
                ),
                patch(
                    "scripts.local_windows_installer._source_branch",
                    return_value=DEVELOPMENT_BRANCH,
                ),
                patch("scripts.local_windows_installer._require_pushed_development_source"),
                patch(
                    "scripts.local_windows_installer.validate_changed_integration_asset_versions",
                    side_effect=DeltaWorkflowError("stale marker"),
                ),
                patch("scripts.local_windows_installer.build") as package,
                self.assertRaisesRegex(
                    LocalInstallerError, "migration validation failed"
                ),
            ):
                candidate(options)
            package.assert_not_called()

    def test_candidate_build_uses_all_selected_jobs_and_only_required_bins(self) -> None:
        arguments = _cargo_build_arguments(Path("C:/cargo-target"), 16)

        self.assertEqual(arguments[arguments.index("--jobs") + 1], "16")
        bins = [
            arguments[index + 1]
            for index, value in enumerate(arguments)
            if value == "--bin"
        ]
        self.assertEqual(bins, ["herdr", "herdr-launcher", "herdr-installer-helper"])

    def test_candidate_normal_focused_test_uses_the_iteration_gate(self) -> None:
        self.assertEqual(
            _just_test_arguments("exact_test_filter"),
            ["test-one", "exact_test_filter"],
        )
        with self.assertRaisesRegex(LocalInstallerError, "non-option test filter"):
            _just_test_arguments("--all")

    def test_candidate_release_focused_test_shares_windows_target_and_jobs(self) -> None:
        cargo_target = Path("C:/cargo-target")
        arguments = _cargo_test_arguments(cargo_target, 16, "exact_test_filter")

        self.assertEqual(arguments[0], "test")
        self.assertIn("--release", arguments)
        self.assertEqual(arguments[arguments.index("--target") + 1], "x86_64-pc-windows-msvc")
        self.assertEqual(arguments[arguments.index("--target-dir") + 1], str(cargo_target))
        self.assertEqual(arguments[arguments.index("--jobs") + 1], "16")
        self.assertEqual(arguments[arguments.index("--bin") + 1], "herdr")
        self.assertEqual(arguments[-3:], ["exact_test_filter", "--", "--nocapture"])

        with self.assertRaisesRegex(LocalInstallerError, "non-option test filter"):
            _cargo_test_arguments(cargo_target, 16, "--all")

    def test_candidate_focused_test_requires_exactly_one_result(self) -> None:
        _require_one_focused_test(
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured\n"
        )

        with self.assertRaisesRegex(LocalInstallerError, "exactly one passing test"):
            _require_one_focused_test(
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured\n"
            )

        _require_one_nextest_test(
            "Summary [   0.038s] 1 test run: 1 passed, 2693 skipped\n"
        )
        with self.assertRaisesRegex(LocalInstallerError, "exactly one passing test"):
            _require_one_nextest_test(
                "Summary [   0.038s] 2 tests run: 2 passed, 2692 skipped\n"
            )

    def test_dynamic_msvc_runtime_imports_are_rejected_case_insensitively(
        self,
    ) -> None:
        dependencies = """
            kernel32.dll
            VCRUNTIME140.dll
            vcruntime140_1.DLL
            MSVCP140.dll
            api-ms-win-crt-runtime-l1-1-0.dll
        """

        self.assertEqual(
            _dynamic_msvc_runtime_imports(dependencies),
            ["MSVCP140.DLL", "VCRUNTIME140.DLL", "VCRUNTIME140_1.DLL"],
        )

    def test_git_commands_trust_only_the_selected_checkout(self) -> None:
        checkout = Path("C:/selected-worktree")

        self.assertEqual(
            _git_arguments(checkout, ["status", "--short"]),
            [
                "-c",
                "core.longpaths=true",
                "-c",
                f"safe.directory={checkout.resolve().as_posix()}",
                "-C",
                str(checkout),
                "status",
                "--short",
            ],
        )


if __name__ == "__main__":
    unittest.main()
