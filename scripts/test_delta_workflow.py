from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Sequence

from scripts.delta_workflow import (
    DEVELOPMENT_BRANCH,
    DEVELOPMENT_REMOTE_REF,
    DeltaWorkflowError,
    _git_command,
    finalize_delta_mailbox,
    materialize_delta_worktree,
    publish_development_worktree,
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
        run_git(
            self.control,
            ["commit", "-m", "feat: first", "-m", "Own the first value."],
        )
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
    def test_git_commands_trust_only_control_and_selected_worktree(self) -> None:
        control = Path("C:/repo/control")
        worktree = Path("C:/repo/development")

        self.assertEqual(
            _git_command(control, ["status", "--short"], cwd=worktree),
            [
                "git",
                "-c",
                "core.longpaths=true",
                "-c",
                f"safe.directory={control.resolve().as_posix()}",
                "-c",
                f"safe.directory={worktree.resolve().as_posix()}",
                "status",
                "--short",
            ],
        )

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

    def test_start_refuses_to_recreate_published_development_from_base(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            origin = Path(temp_dir) / "origin.git"
            origin.mkdir()
            run_git(origin, ["init", "--bare"])
            run_git(fixture.control, ["remote", "add", "origin", str(origin)])
            run_git(
                fixture.control,
                ["push", "origin", f"{fixture.base}:{DEVELOPMENT_REMOTE_REF}"],
            )
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()

            with self.assertRaisesRegex(
                DeltaWorkflowError, "remote cumulative development branch already exists"
            ):
                start_delta_worktree(
                    "development", worktrees / "development", fixture.control
                )

    def test_materialize_replays_an_existing_task_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "managed"
            run_git(
                fixture.control,
                [
                    "worktree",
                    "add",
                    "-b",
                    "agent/delta-managed",
                    str(worktree),
                    fixture.base,
                ],
            )

            result = materialize_delta_worktree(worktree, fixture.control)

            self.assertEqual(result.branch, "agent/delta-managed")
            self.assertEqual(result.tree, fixture.source_tree)
            self.assertEqual(
                run_git(worktree, ["rev-list", "--count", f"{fixture.base}..HEAD"]),
                "2",
            )

    def test_publish_development_pushes_only_the_exact_shared_branch(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            origin = Path(temp_dir) / "origin.git"
            origin.mkdir()
            run_git(origin, ["init", "--bare"])
            run_git(fixture.control, ["remote", "add", "origin", str(origin)])
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "development"
            started = start_delta_worktree("development", worktree, fixture.control)
            self.assertEqual(started.branch, DEVELOPMENT_BRANCH)

            result = publish_development_worktree(worktree, fixture.control)

            self.assertEqual(result.tree, fixture.source_tree)
            self.assertEqual(
                run_git(
                    fixture.control,
                    [
                        "ls-remote",
                        "--heads",
                        "origin",
                        DEVELOPMENT_REMOTE_REF,
                    ],
                ).split()[0],
                result.head,
            )

            topic = worktrees / "topic"
            run_git(
                fixture.control,
                [
                    "worktree",
                    "add",
                    "-b",
                    "agent/delta-topic",
                    str(topic),
                    result.head,
                ],
            )
            run_git(topic, ["config", "user.name", "Delta Test"])
            run_git(topic, ["config", "user.email", "delta@example.invalid"])
            (topic / "topic.txt").write_bytes(b"finished topic\n")
            run_git(topic, ["add", "topic.txt"])
            run_git(topic, ["commit", "-m", "topic change"])

            with self.assertRaisesRegex(
                DeltaWorkflowError, "unintegrated topic worktrees"
            ):
                publish_development_worktree(worktree, fixture.control)

            run_git(worktree, ["merge", "--ff-only", "agent/delta-topic"])
            publish_development_worktree(worktree, fixture.control)

    def test_finalize_updates_only_owner_and_reproduces_tested_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "finalize"
            start_delta_worktree("finalize", worktree, fixture.control)

            (worktree / "value.txt").write_bytes(b"final\n")
            run_git(worktree, ["add", "value.txt"])
            run_git(worktree, ["commit", "-m", "fix: finalize value"])
            source_head = run_git(worktree, ["rev-parse", "HEAD"])
            source_tree = run_git(worktree, ["rev-parse", "HEAD^{tree}"])
            delta_root = fixture.control / "patches" / "delta"
            later_mailbox = delta_root / "0002-second.patch"
            later_bytes = later_mailbox.read_bytes()

            result = finalize_delta_mailbox(
                worktree,
                "0001-first.patch",
                source_tree,
                fixture.control,
            )

            self.assertEqual(result.source_head, source_head)
            self.assertEqual(result.source_tree, source_tree)
            self.assertEqual(result.replay_tree, source_tree)
            self.assertEqual(run_git(worktree, ["rev-parse", "HEAD"]), source_head)
            self.assertEqual(later_mailbox.read_bytes(), later_bytes)
            self.assertIn(
                "Subject: [PATCH 1/2] feat: first",
                (delta_root / "0001-first.patch").read_text(encoding="utf-8"),
            )
            self.assertIn(
                "Own the first value.",
                (delta_root / "0001-first.patch").read_text(encoding="utf-8"),
            )
            self.assertEqual(
                verify_replay_tree(source_tree, fixture.control).tree,
                source_tree,
            )

    def test_finalize_appends_one_new_mailbox_and_renumbers_series(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "new-mailbox"
            start_delta_worktree("new-mailbox", worktree, fixture.control)

            (worktree / "third.txt").write_bytes(b"third\n")
            run_git(worktree, ["add", "third.txt"])
            run_git(
                worktree,
                ["commit", "-m", "feat: third", "-m", "Refs example/repo#123"],
            )
            source_head = run_git(worktree, ["rev-parse", "HEAD"])
            source_tree = run_git(worktree, ["rev-parse", "HEAD^{tree}"])

            result = finalize_delta_mailbox(
                worktree,
                "0003-third.patch",
                source_tree,
                fixture.control,
                new_mailbox=True,
            )

            delta_root = fixture.control / "patches" / "delta"
            self.assertEqual(result.source_head, source_head)
            self.assertEqual(result.replay_tree, source_tree)
            self.assertEqual(
                (delta_root / "series").read_text(encoding="utf-8").splitlines(),
                ["0001-first.patch", "0002-second.patch", "0003-third.patch"],
            )
            self.assertIn(
                "Subject: [PATCH 1/3] feat: first",
                (delta_root / "0001-first.patch").read_text(encoding="utf-8"),
            )
            self.assertIn(
                "Subject: [PATCH 2/3] feat: second",
                (delta_root / "0002-second.patch").read_text(encoding="utf-8"),
            )
            new_mailbox = (delta_root / "0003-third.patch").read_text(
                encoding="utf-8"
            )
            self.assertIn("Subject: [PATCH 3/3] feat: third", new_mailbox)
            self.assertIn("Refs example/repo#123", new_mailbox)
            self.assertEqual(
                verify_replay_tree(source_tree, fixture.control).tree,
                source_tree,
            )

    def test_finalize_rejects_a_tree_not_at_source_head(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fixture = DeltaFixture(Path(temp_dir))
            worktrees = Path(temp_dir) / "worktrees"
            worktrees.mkdir()
            worktree = worktrees / "mismatch"
            start_delta_worktree("mismatch", worktree, fixture.control)

            (worktree / "value.txt").write_bytes(b"final\n")
            run_git(worktree, ["add", "value.txt"])
            run_git(worktree, ["commit", "-m", "fix: finalize value"])
            mailbox = fixture.control / "patches" / "delta" / "0001-first.patch"
            original = mailbox.read_bytes()

            with self.assertRaisesRegex(
                DeltaWorkflowError, "changed after its tested tree was recorded"
            ):
                finalize_delta_mailbox(
                    worktree,
                    "0001-first.patch",
                    fixture.source_tree,
                    fixture.control,
                )

            self.assertEqual(mailbox.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
