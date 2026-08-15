from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from scripts.local_windows_installer import (
    InstallerIdentity,
    LocalInstallerError,
    OUTPUT_PATH,
    _bundle_manifest,
    _hashes,
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


if __name__ == "__main__":
    unittest.main()
