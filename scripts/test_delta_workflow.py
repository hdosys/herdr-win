from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Sequence

from scripts.delta_workflow import (
    DeltaWorkflowError,
    start_delta_worktree,
    verify_replay_tree,
)


GIT_TIMEOUT_SECONDS = 30


def run_git(
    cwd: Path,
    arguments: Sequence[str],
    *,
    strip: bool = True,
) -> str:
    result = subprocess.run(
        ["git", *arguments],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr or result.stdout)
    return result.stdout.strip() if strip else result.stdout


class DeltaFixture:
    def __init__(self, root: Path) -> None:
        self.control = root / "control"
        self.control.mkdir()
        run_git(self.control, ["init", "-b", "master"])
        run_git(self.control, ["config", "user.name", "Delta Test"])
        run_git(self.control, ["config", "user.email", "delta@example.com"])
        run_git(self.control, ["config", "core.autocrlf", "false"])

        (self.control / "value.txt").write_bytes(b"base\n")
        run_git(self.control, ["add", "value.txt"])
        run_git(self.control, ["commit", "-m", "base"])
        self.base = run_git(self.control, ["rev-parse", "HEAD"])

        (self.control / "value.txt").write_bytes(b"first\n")
        run_git(self.control, ["add", "value.txt"])
        run_git(self.control, ["commit", "-m", "feat: first"])
        first = run_git(self.control, ["rev-parse", "HEAD"])
        first_patch = run_git(
            self.control,
            [
                "format-patch",
                "--stdout",
                "--full-index",
                "--binary",
                "--subject-prefix=PATCH 1/2",
                "-1",
                first,
            ],
            strip=False,
        )

        (self.control / "second.txt").write_bytes(b"second\n")
        run_git(self.control, ["add", "second.txt"])
        run_git(self.control, ["commit", "-m", "feat: second"])
        second = run_git(self.control, ["rev-parse", "HEAD"])
        second_patch = run_git(
            self.control,
            [
                "format-patch",
                "--stdout",
                "--full-index",
                "--binary",
                "--subject-prefix=PATCH 2/2",
                "-1",
                second,
            ],
            strip=False,
        )
        self.source_tree = run_git(self.control, ["rev-parse", "HEAD^{tree}"])

        delta_root = self.control / "patches" / "delta"
        delta_root.mkdir(parents=True)
        (delta_root / "BASE").write_bytes(f"{self.base}\n".encode())
        (delta_root / "series").write_bytes(
            b"0001-first.patch\n0002-second.patch\n"
        )
        (delta_root / "0001-first.patch").write_text(
            first_patch, encoding="utf-8", newline="\n"
        )
        (delta_root / "0002-second.patch").write_text(
            second_patch, encoding="utf-8", newline="\n"
        )
        run_git(self.control, ["add", "patches/delta"])
        run_git(self.control, ["commit", "-m", "chore: add delta"])


class DeltaWorkflowTests(unittest.TestCase):
    def test_temporary_index_replay_matches_the_source_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))

            result = verify_replay_tree(fixture.source_tree, fixture.control)

            self.assertEqual(result.base, fixture.base)
            self.assertEqual(
                result.mailboxes, ("0001-first.patch", "0002-second.patch")
            )
            self.assertEqual(result.tree, fixture.source_tree)
            self.assertEqual(run_git(fixture.control, ["status", "--porcelain"]), "")

    def test_expected_tree_mismatch_reports_the_semantic_difference(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            base_tree = run_git(
                fixture.control, ["rev-parse", f"{fixture.base}^{{tree}}"]
            )

            with self.assertRaisesRegex(
                DeltaWorkflowError, "does not match the tested source tree"
            ):
                verify_replay_tree(base_tree, fixture.control)

    def test_start_creates_one_replayed_task_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "fast-path"

            result = start_delta_worktree("fast-path", worktree, fixture.control)

            self.assertEqual(result.branch, "agent/delta-fast-path")
            self.assertEqual(result.mailbox_count, 2)
            self.assertEqual(result.tree, fixture.source_tree)
            self.assertEqual(
                run_git(worktree, ["rev-list", "--count", f"{fixture.base}..HEAD"]),
                "2",
            )


if __name__ == "__main__":
    unittest.main()
