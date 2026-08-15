#!/usr/bin/env python3
"""Fast local iteration and tree-exact verification for the maintained delta."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


PROJECT_ROOT = Path(__file__).resolve().parent.parent
BASE_RE = re.compile(r"^[0-9a-f]{40}$")
PATCH_RE = re.compile(r"^[0-9]{4}-[a-z0-9-]+\.patch$")
TASK_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,39}$")
GIT_TIMEOUT_SECONDS = 120


class DeltaWorkflowError(RuntimeError):
    """A bounded delta workflow operation could not complete safely."""


@dataclass(frozen=True)
class ReplayResult:
    base: str
    mailboxes: tuple[str, ...]
    tree: str


@dataclass(frozen=True)
class WorktreeResult:
    branch: str
    path: Path
    base: str
    head: str
    tree: str
    mailbox_count: int


def _git_environment(overrides: dict[str, str] | None = None) -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_MERGE_AUTOEDIT": "no",
        }
    )
    if overrides:
        environment.update(overrides)
    return environment


def _run_git(
    project_root: Path,
    arguments: Sequence[str],
    *,
    cwd: Path | None = None,
    environment: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    command = ["git", "-c", "core.longpaths=true", *arguments]
    try:
        result = subprocess.run(
            command,
            cwd=cwd or project_root,
            env=_git_environment(environment),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=GIT_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise DeltaWorkflowError(
            f"could not run {' '.join(command)!r}: {error}"
        ) from error

    if check and result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise DeltaWorkflowError(
            f"{' '.join(command)!r} failed with exit code {result.returncode}: {detail}"
        )
    return result


def _read_base(project_root: Path) -> str:
    path = project_root / "patches" / "delta" / "BASE"
    try:
        base = path.read_text(encoding="utf-8").strip()
    except OSError as error:
        raise DeltaWorkflowError(f"could not read {path}: {error}") from error
    if BASE_RE.fullmatch(base) is None:
        raise DeltaWorkflowError(f"{path} must contain one full lowercase commit ID")
    _run_git(project_root, ["cat-file", "-e", f"{base}^{{commit}}"])
    return base


def _read_series(project_root: Path) -> tuple[str, ...]:
    delta_root = project_root / "patches" / "delta"
    path = delta_root / "series"
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise DeltaWorkflowError(f"could not read {path}: {error}") from error

    mailboxes: list[str] = []
    for line_number, raw_line in enumerate(lines, start=1):
        entry = raw_line.strip()
        if not entry or entry.startswith("#"):
            continue
        if PATCH_RE.fullmatch(entry) is None:
            raise DeltaWorkflowError(
                f"{path}:{line_number} contains invalid mailbox name {entry!r}"
            )
        if not (delta_root / entry).is_file():
            raise DeltaWorkflowError(f"mailbox {delta_root / entry} does not exist")
        mailboxes.append(entry)

    if not mailboxes:
        raise DeltaWorkflowError(f"{path} does not list any mailboxes")
    if len(mailboxes) != len(set(mailboxes)):
        raise DeltaWorkflowError(f"{path} contains duplicate mailbox names")
    return tuple(mailboxes)


def replay_delta_tree(project_root: Path = PROJECT_ROOT) -> ReplayResult:
    """Apply the checked-in queue to a temporary index and return its tree ID."""

    project_root = project_root.resolve()
    base = _read_base(project_root)
    mailboxes = _read_series(project_root)
    delta_root = project_root / "patches" / "delta"

    with tempfile.TemporaryDirectory(prefix="herdr-delta-index-") as temp_dir:
        index_path = Path(temp_dir) / "index"
        index_environment = {"GIT_INDEX_FILE": str(index_path)}
        _run_git(project_root, ["read-tree", base], environment=index_environment)
        for mailbox in mailboxes:
            _run_git(
                project_root,
                [
                    "apply",
                    "--cached",
                    "--3way",
                    "--whitespace=nowarn",
                    str(delta_root / mailbox),
                ],
                environment=index_environment,
            )
        tree = _run_git(
            project_root, ["write-tree"], environment=index_environment
        ).stdout.strip()

    if BASE_RE.fullmatch(tree) is None:
        raise DeltaWorkflowError(f"git write-tree returned invalid tree ID {tree!r}")
    _run_git(project_root, ["diff", "--check", base, tree])
    return ReplayResult(base=base, mailboxes=mailboxes, tree=tree)


def verify_replay_tree(
    expected_tree: str | None,
    project_root: Path = PROJECT_ROOT,
) -> ReplayResult:
    """Replay the queue and optionally require one exact, previously tested tree."""

    project_root = project_root.resolve()
    if expected_tree is not None:
        if BASE_RE.fullmatch(expected_tree) is None:
            raise DeltaWorkflowError(
                "--expected-tree must be one full lowercase Git tree ID"
            )
        object_type = _run_git(
            project_root, ["cat-file", "-t", expected_tree]
        ).stdout.strip()
        if object_type != "tree":
            raise DeltaWorkflowError(
                f"--expected-tree identifies a {object_type}, not a tree"
            )

    result = replay_delta_tree(project_root)
    if expected_tree is not None and result.tree != expected_tree:
        difference = _run_git(
            project_root,
            ["diff", "--stat", expected_tree, result.tree],
            check=False,
        ).stdout.strip()
        detail = f"\n{difference}" if difference else ""
        raise DeltaWorkflowError(
            "replayed delta tree does not match the tested source tree: "
            f"expected {expected_tree}, found {result.tree}{detail}"
        )
    return result


def _require_clean_delta(project_root: Path) -> None:
    status = _run_git(
        project_root,
        [
            "status",
            "--porcelain=v1",
            "--untracked-files=no",
            "--",
            "patches/delta",
        ],
    ).stdout.strip()
    if status:
        raise DeltaWorkflowError(
            "cannot start from modified delta inputs; finalize or preserve them first:\n"
            f"{status}"
        )


def start_delta_worktree(
    name: str,
    path: Path,
    project_root: Path = PROJECT_ROOT,
) -> WorktreeResult:
    """Create one task worktree and replay the queue into it exactly once."""

    project_root = project_root.resolve()
    if TASK_RE.fullmatch(name) is None:
        raise DeltaWorkflowError(
            "task name must use 1 to 40 lowercase letters, digits, or hyphens"
        )
    if not path.is_absolute():
        raise DeltaWorkflowError("worktree path must be absolute")
    path = path.resolve()
    try:
        path.relative_to(project_root)
    except ValueError:
        pass
    else:
        raise DeltaWorkflowError("worktree path must be outside the control checkout")
    if path.exists():
        raise DeltaWorkflowError(f"worktree path already exists: {path}")
    if not path.parent.is_dir():
        raise DeltaWorkflowError(
            f"worktree parent directory does not exist: {path.parent}"
        )

    current_branch = _run_git(
        project_root, ["symbolic-ref", "--short", "HEAD"]
    ).stdout.strip()
    if current_branch != "master":
        raise DeltaWorkflowError(
            f"control checkout must be on master, found {current_branch!r}"
        )
    _require_clean_delta(project_root)

    base = _read_base(project_root)
    mailboxes = _read_series(project_root)
    branch = f"agent/delta-{name}"
    _run_git(project_root, ["check-ref-format", "--branch", branch])
    branch_exists = _run_git(
        project_root,
        ["show-ref", "--verify", "--quiet", f"refs/heads/{branch}"],
        check=False,
    )
    if branch_exists.returncode == 0:
        raise DeltaWorkflowError(f"worktree branch already exists: {branch}")
    if branch_exists.returncode not in (0, 1):
        detail = branch_exists.stderr.strip() or "could not inspect branch"
        raise DeltaWorkflowError(detail)

    _run_git(project_root, ["worktree", "add", "-b", branch, str(path), base])
    replay_environment = {
        "GIT_COMMITTER_NAME": "herdr-win replay",
        "GIT_COMMITTER_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
    }
    try:
        for mailbox in mailboxes:
            _run_git(
                project_root,
                [
                    "am",
                    "--3way",
                    str(project_root / "patches" / "delta" / mailbox),
                ],
                cwd=path,
                environment=replay_environment,
            )
        _run_git(path, ["diff", "--check", f"{base}..HEAD"], cwd=path)
    except DeltaWorkflowError as error:
        raise DeltaWorkflowError(
            f"delta replay stopped in {path}; preserve and inspect that worktree: {error}"
        ) from error

    head = _run_git(path, ["rev-parse", "HEAD"], cwd=path).stdout.strip()
    tree = _run_git(path, ["rev-parse", "HEAD^{tree}"], cwd=path).stdout.strip()
    return WorktreeResult(
        branch=branch,
        path=path,
        base=base,
        head=head,
        tree=tree,
        mailbox_count=len(mailboxes),
    )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Materialize and verify the maintained herdr-win delta."
    )
    commands = parser.add_subparsers(dest="command", required=True)

    start = commands.add_parser(
        "start", help="create one task worktree and replay the queue once"
    )
    start.add_argument("--name", required=True, help="lowercase task slug")
    start.add_argument(
        "--path",
        required=True,
        type=Path,
        help="absolute path below an existing parent",
    )

    check = commands.add_parser(
        "check", help="replay into a temporary index without another checkout"
    )
    check.add_argument(
        "--expected-tree",
        help="exact tested Git tree ID that the checked-in queue must reproduce",
    )
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    options = parser.parse_args(arguments)
    try:
        if options.command == "start":
            result = start_delta_worktree(options.name, options.path)
            print(f"worktree: {result.path}")
            print(f"branch: {result.branch}")
            print(f"base: {result.base}")
            print(f"head: {result.head}")
            print(f"tree: {result.tree}")
            print(f"mailboxes: {result.mailbox_count}")
            return 0

        result = verify_replay_tree(options.expected_tree)
        print(f"base: {result.base}")
        print(f"tree: {result.tree}")
        print(f"mailboxes: {len(result.mailboxes)}")
        if options.expected_tree is not None:
            print("tested-tree-match: yes")
        return 0
    except DeltaWorkflowError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
