from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.readme_mermaid_check import (
    MermaidCheckError,
    _renderer_command,
    extract_mermaid,
    render_mermaid,
    require_balanced_aspect,
)


class ReadmeMermaidCheckTests(unittest.TestCase):
    def test_renderer_command_resolves_the_platform_executable(self) -> None:
        with mock.patch.dict("os.environ", {"MERMAID_CLI": "mmdc"}):
            with mock.patch(
                "scripts.readme_mermaid_check.shutil.which",
                return_value=r"C:\tools\mmdc.cmd",
            ):
                self.assertEqual(_renderer_command(), [r"C:\tools\mmdc.cmd"])

    @unittest.skipUnless(sys.platform == "win32", "Windows command parsing contract")
    def test_renderer_command_unquotes_windows_executable_path(self) -> None:
        configured = '"C:\\Program Files\\nodejs\\npx.cmd" --yes mmdc'
        with mock.patch.dict("os.environ", {"MERMAID_CLI": configured}):
            self.assertEqual(
                _renderer_command(),
                [r"C:\Program Files\nodejs\npx.cmd", "--yes", "mmdc"],
            )

    def test_extracts_exactly_one_mermaid_block(self) -> None:
        self.assertEqual(
            extract_mermaid("before\n```mermaid\nflowchart LR\n  A --> B\n```\nafter\n"),
            "flowchart LR\n  A --> B\n",
        )
        with self.assertRaisesRegex(MermaidCheckError, "exactly one"):
            extract_mermaid("no diagram")

    def test_rejects_unbalanced_render_dimensions(self) -> None:
        self.assertAlmostEqual(require_balanced_aspect(622, 484), 622 / 484)
        with self.assertRaisesRegex(MermaidCheckError, "unreadable"):
            require_balanced_aspect(1200, 200)
        with self.assertRaisesRegex(MermaidCheckError, "unreadable"):
            require_balanced_aspect(200, 1200)

    def test_renderer_output_owns_the_aspect_check(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fake = Path(temporary) / "fake_renderer.py"
            fake.write_text(
                "import pathlib, sys\n"
                "output = pathlib.Path(sys.argv[sys.argv.index('-o') + 1])\n"
                "output.write_text('<svg viewBox=\"0 0 622 484\"></svg>', encoding='utf-8')\n",
                encoding="utf-8",
            )

            width, height, ratio = render_mermaid(
                "flowchart LR\n  A --> B\n", [sys.executable, str(fake)]
            )

            self.assertEqual((width, height), (622, 484))
            self.assertAlmostEqual(ratio, 622 / 484)


if __name__ == "__main__":
    unittest.main()
