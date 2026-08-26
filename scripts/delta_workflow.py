#!/usr/bin/env python3
"""Materialize, finalize, and verify the maintained delta."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from email import policy
from email.parser import BytesParser
from email.utils import parseaddr
from pathlib import Path
from typing import Sequence


PROJECT_ROOT = Path(__file__).resolve().parent.parent
BASE_RE = re.compile(r"^[0-9a-f]{40}$")
PATCH_RE = re.compile(r"^[0-9]{4}-[a-z0-9-]+\.patch$")
TASK_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,39}$")
GIT_TIMEOUT_SECONDS = 120
PREFIX_COMPILE_TIMEOUT_SECONDS = 1200
DEVELOPMENT_BRANCH = "candidate/development"
DEVELOPMENT_REMOTE_REF = f"refs/heads/{DEVELOPMENT_BRANCH}"


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


@dataclass(frozen=True)
class FinalizeResult:
    mailbox: str
    source_head: str
    source_tree: str
    replay_tree: str


@dataclass(frozen=True)
class DevelopmentResult:
    path: Path
    head: str
    tree: str


@dataclass(frozen=True)
class TopicWorktree:
    path: Path
    head: str
    branch: str


@dataclass(frozen=True)
class MailboxMetadata:
    author_name: str
    author_email: str
    author_date: str
    commit_message: str


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
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    command = _git_command(project_root, arguments, cwd=cwd)
    try:
        result = subprocess.run(
            command,
            cwd=cwd or project_root,
            env=_git_environment(environment),
            input=input_text,
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


def _git_command(
    project_root: Path,
    arguments: Sequence[str],
    *,
    cwd: Path | None = None,
) -> list[str]:
    command = ["git", "-c", "core.longpaths=true"]
    trusted: list[Path] = []
    for path in (project_root, cwd or project_root):
        resolved = path.resolve()
        if resolved not in trusted:
            trusted.append(resolved)
            command.extend(["-c", f"safe.directory={resolved.as_posix()}"])
    command.extend(arguments)
    return command


def unintegrated_topic_worktrees(
    project_root: Path,
    development_head: str,
) -> tuple[TopicWorktree, ...]:
    """Return linked topic heads not contained in the development head."""

    output = _run_git(
        project_root,
        ["worktree", "list", "--porcelain", "-z"],
    ).stdout
    linked: list[TopicWorktree] = []
    path: Path | None = None
    head = ""
    branch = ""
    for field in output.split("\0"):
        if field.startswith("worktree "):
            if path is not None and head and branch:
                linked.append(TopicWorktree(path, head, branch))
            path = Path(field.removeprefix("worktree ")).resolve()
            head = ""
            branch = ""
        elif field.startswith("HEAD "):
            head = field.removeprefix("HEAD ")
        elif field.startswith("branch refs/heads/"):
            branch = field.removeprefix("branch refs/heads/")
    if path is not None and head and branch:
        linked.append(TopicWorktree(path, head, branch))

    unintegrated: list[TopicWorktree] = []
    for worktree in linked:
        if not worktree.branch.startswith("agent/delta-"):
            continue
        ancestor = _run_git(
            project_root,
            ["merge-base", "--is-ancestor", worktree.head, development_head],
            check=False,
        )
        if ancestor.returncode != 0:
            unintegrated.append(worktree)
    return tuple(unintegrated)


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


def _tree_after_patches(
    project_root: Path,
    initial_tree: str,
    patch_paths: Sequence[Path],
    *,
    reverse: bool = False,
) -> str:
    with tempfile.TemporaryDirectory(prefix="herdr-delta-index-") as temp_dir:
        index_path = Path(temp_dir) / "index"
        index_environment = {"GIT_INDEX_FILE": str(index_path)}
        _run_git(
            project_root,
            ["read-tree", initial_tree],
            environment=index_environment,
        )
        for patch_path in patch_paths:
            arguments = [
                "apply",
                "--cached",
                "--3way",
                "--whitespace=nowarn",
            ]
            if reverse:
                arguments.append("--reverse")
            arguments.append(str(patch_path))
            _run_git(project_root, arguments, environment=index_environment)
        tree = _run_git(
            project_root, ["write-tree"], environment=index_environment
        ).stdout.strip()

    if BASE_RE.fullmatch(tree) is None:
        raise DeltaWorkflowError(f"git write-tree returned invalid tree ID {tree!r}")
    return tree


def replay_delta_tree(project_root: Path = PROJECT_ROOT) -> ReplayResult:
    """Apply the checked-in queue to a temporary index and return its tree ID."""

    project_root = project_root.resolve()
    base = _read_base(project_root)
    mailboxes = _read_series(project_root)
    delta_root = project_root / "patches" / "delta"
    tree = _tree_after_patches(
        project_root,
        base,
        [delta_root / mailbox for mailbox in mailboxes],
    )
    _run_git(project_root, ["diff", "--check", base, tree])
    return ReplayResult(base=base, mailboxes=mailboxes, tree=tree)


def compile_delta_prefixes(
    project_root: Path = PROJECT_ROOT,
    *,
    check_command: Sequence[str] = ("cargo", "check", "--locked", "--bins"),
) -> tuple[str, ...]:
    """Replay and compile each ordered mailbox prefix for an explicit refresh."""

    project_root = project_root.resolve()
    if not check_command:
        raise DeltaWorkflowError("prefix compile command must not be empty")
    base = _read_base(project_root)
    mailboxes = _read_series(project_root)
    replay_environment = {
        "GIT_COMMITTER_NAME": "herdr-win replay",
        "GIT_COMMITTER_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
    }
    with tempfile.TemporaryDirectory(prefix="herdr-delta-prefixes-") as temp_dir:
        temporary = Path(temp_dir)
        checkout = temporary / "source"
        _run_git(
            project_root,
            ["clone", "--shared", "--no-checkout", str(project_root), str(checkout)],
        )
        _run_git(project_root, ["checkout", "--detach", base], cwd=checkout)
        environment = os.environ.copy()
        environment.update(
            {
                "CARGO_BUILD_JOBS": str(os.cpu_count() or 1),
                "CARGO_TARGET_DIR": str(temporary / "target"),
                "GIT_TERMINAL_PROMPT": "0",
            }
        )
        for prefix, mailbox in enumerate(mailboxes, start=1):
            _run_git(
                project_root,
                ["am", "--3way", str(project_root / "patches" / "delta" / mailbox)],
                cwd=checkout,
                environment=replay_environment,
            )
            environment["HERDR_DELTA_PREFIX"] = str(prefix)
            environment["HERDR_DELTA_MAILBOX"] = mailbox
            try:
                result = subprocess.run(
                    list(check_command),
                    cwd=checkout,
                    env=environment,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    check=False,
                    timeout=PREFIX_COMPILE_TIMEOUT_SECONDS,
                )
            except (OSError, subprocess.TimeoutExpired) as error:
                raise DeltaWorkflowError(
                    f"could not compile delta prefix through {mailbox}: {error}"
                ) from error
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
                raise DeltaWorkflowError(
                    f"delta prefix through {mailbox} failed to compile: {detail}"
                )
    return mailboxes


def _require_tree_object(project_root: Path, tree: str, label: str) -> None:
    if BASE_RE.fullmatch(tree) is None:
        raise DeltaWorkflowError(f"{label} must be one full lowercase Git tree ID")
    object_type = _run_git(project_root, ["cat-file", "-t", tree]).stdout.strip()
    if object_type != "tree":
        raise DeltaWorkflowError(f"{label} identifies a {object_type}, not a tree")


def verify_replay_tree(
    expected_tree: str | None,
    project_root: Path = PROJECT_ROOT,
) -> ReplayResult:
    """Replay the queue and optionally require one exact, previously tested tree."""

    project_root = project_root.resolve()
    if expected_tree is not None:
        _require_tree_object(project_root, expected_tree, "--expected-tree")

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
            "delta inputs must be clean before this operation:\n"
            f"{status}"
        )


def _require_control_master(project_root: Path) -> None:
    current_branch = _run_git(
        project_root, ["symbolic-ref", "--short", "HEAD"]
    ).stdout.strip()
    if current_branch != "master":
        raise DeltaWorkflowError(
            f"control checkout must be on master, found {current_branch!r}"
        )


def _normalized_git_path(path: str) -> str:
    return os.path.normcase(os.path.realpath(path))


def _require_clean_delta_worktree(
    project_root: Path,
    worktree: Path,
) -> tuple[Path, str]:
    if not worktree.is_absolute():
        raise DeltaWorkflowError("--worktree must be an absolute path")
    worktree = worktree.resolve()
    if not worktree.is_dir():
        raise DeltaWorkflowError(f"source worktree does not exist: {worktree}")

    source_root = Path(
        _run_git(worktree, ["rev-parse", "--show-toplevel"], cwd=worktree)
        .stdout.strip()
    ).resolve()
    if source_root != worktree:
        raise DeltaWorkflowError(
            f"--worktree must identify its Git root, found {source_root}"
        )

    control_common = _run_git(
        project_root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ).stdout.strip()
    source_common = _run_git(
        worktree,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=worktree,
    ).stdout.strip()
    if _normalized_git_path(control_common) != _normalized_git_path(source_common):
        raise DeltaWorkflowError("source worktree belongs to another Git repository")

    branch = _run_git(
        worktree, ["symbolic-ref", "--short", "HEAD"], cwd=worktree
    ).stdout.strip()
    if branch != DEVELOPMENT_BRANCH and not branch.startswith("agent/delta-"):
        raise DeltaWorkflowError(
            "source worktree must use the cumulative development branch or an "
            f"agent/delta-* topic branch, found {branch!r}"
        )

    status = _run_git(
        worktree,
        ["status", "--porcelain=v1", "--untracked-files=all"],
        cwd=worktree,
    ).stdout.strip()
    if status:
        raise DeltaWorkflowError(f"source worktree must be clean:\n{status}")

    for marker in (
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ):
        marker_path = _run_git(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-path", marker],
            cwd=worktree,
        ).stdout.strip()
        if Path(marker_path).exists():
            raise DeltaWorkflowError(
                f"source worktree has an in-progress Git operation: {marker}"
            )
    return worktree, branch


def _require_source_worktree(
    project_root: Path,
    worktree: Path,
    expected_tree: str,
    replay: ReplayResult,
) -> tuple[str, str]:
    worktree, _ = _require_clean_delta_worktree(project_root, worktree)

    ancestor = _run_git(
        worktree,
        ["merge-base", "--is-ancestor", replay.base, "HEAD"],
        cwd=worktree,
        check=False,
    )
    if ancestor.returncode != 0:
        raise DeltaWorkflowError("recorded BASE is not an ancestor of source HEAD")
    merges = _run_git(
        worktree,
        ["rev-list", "--merges", f"{replay.base}..HEAD"],
        cwd=worktree,
    ).stdout.strip()
    if merges:
        raise DeltaWorkflowError("source worktree history must be linear")

    commits = _run_git(
        worktree,
        ["rev-list", "--reverse", f"{replay.base}..HEAD"],
        cwd=worktree,
    ).stdout.splitlines()
    if len(commits) <= len(replay.mailboxes):
        raise DeltaWorkflowError("source worktree has no committed WIP change")
    queue_head = commits[len(replay.mailboxes) - 1]
    queue_tree = _run_git(
        worktree, ["rev-parse", f"{queue_head}^{{tree}}"], cwd=worktree
    ).stdout.strip()
    if queue_tree != replay.tree:
        raise DeltaWorkflowError(
            "source worktree was not started from the current checked-in queue"
        )

    source_head = _run_git(worktree, ["rev-parse", "HEAD"], cwd=worktree).stdout.strip()
    source_tree = _run_git(
        worktree, ["rev-parse", "HEAD^{tree}"], cwd=worktree
    ).stdout.strip()
    if source_tree != expected_tree:
        raise DeltaWorkflowError(
            "source worktree changed after its tested tree was recorded: "
            f"expected {expected_tree}, found {source_tree}"
        )
    if source_tree == replay.tree:
        raise DeltaWorkflowError("source worktree has no net tree change to finalize")
    return source_head, source_tree


def _read_mailbox_metadata(
    path: Path,
    position: int,
    mailbox_count: int,
) -> MailboxMetadata:
    try:
        message = BytesParser(policy=policy.default).parsebytes(path.read_bytes())
    except (OSError, UnicodeError) as error:
        raise DeltaWorkflowError(f"could not parse mailbox {path}: {error}") from error

    subject = str(message.get("Subject", "")).strip()
    expected_prefix = f"[PATCH {position}/{mailbox_count}] "
    if not subject.startswith(expected_prefix):
        raise DeltaWorkflowError(
            f"{path} subject must start with {expected_prefix!r}, found {subject!r}"
        )
    commit_subject = subject[len(expected_prefix) :].strip()
    author_name, author_email = parseaddr(str(message.get("From", "")))
    author_date = str(message.get("Date", "")).strip()
    if not commit_subject or not author_name or not author_email or not author_date:
        raise DeltaWorkflowError(f"{path} has incomplete commit metadata")

    payload = message.get_payload(decode=True)
    if not isinstance(payload, bytes):
        raise DeltaWorkflowError(f"{path} must contain one plain-text patch payload")
    text = payload.decode("utf-8", errors="strict").replace("\r\n", "\n")
    lines = text.splitlines()
    try:
        separator_index = lines.index("---")
    except ValueError:
        raise DeltaWorkflowError(f"{path} is missing the format-patch separator")
    body = "\n".join(lines[:separator_index]).strip("\n")
    commit_message = commit_subject
    if body:
        commit_message = f"{commit_message}\n\n{body}"
    return MailboxMetadata(
        author_name=author_name,
        author_email=author_email,
        author_date=author_date,
        commit_message=f"{commit_message}\n",
    )


def _read_commit_metadata(worktree: Path, commit: str) -> MailboxMetadata:
    fields = _run_git(
        worktree,
        ["show", "-s", "--format=%an%x00%ae%x00%aI%x00%B", commit],
        cwd=worktree,
    ).stdout.split("\0", 3)
    if len(fields) != 4:
        raise DeltaWorkflowError(f"could not read source commit metadata from {commit}")
    author_name, author_email, author_date, commit_message = fields
    commit_message = commit_message.rstrip("\n")
    if not author_name or not author_email or not author_date or not commit_message:
        raise DeltaWorkflowError(f"source commit {commit} has incomplete metadata")
    return MailboxMetadata(
        author_name=author_name,
        author_email=author_email,
        author_date=author_date,
        commit_message=f"{commit_message}\n",
    )


def _commit_tree(
    project_root: Path,
    tree: str,
    parent: str,
    message: str,
    environment: dict[str, str],
) -> str:
    return _run_git(
        project_root,
        ["commit-tree", tree, "-p", parent],
        environment=environment,
        input_text=message,
    ).stdout.strip()


def _candidate_mailbox(
    project_root: Path,
    prefix_tree: str,
    owner_tree: str,
    metadata: MailboxMetadata,
    position: int,
    mailbox_count: int,
) -> str:
    replay_identity = {
        "GIT_AUTHOR_NAME": "herdr-win replay",
        "GIT_AUTHOR_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
        "GIT_COMMITTER_NAME": "herdr-win replay",
        "GIT_COMMITTER_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
    }
    prefix_commit = _commit_tree(
        project_root,
        prefix_tree,
        _read_base(project_root),
        "herdr-win delta prefix\n",
        replay_identity,
    )
    owner_environment = {
        **replay_identity,
        "GIT_AUTHOR_NAME": metadata.author_name,
        "GIT_AUTHOR_EMAIL": metadata.author_email,
        "GIT_AUTHOR_DATE": metadata.author_date,
    }
    owner_commit = _commit_tree(
        project_root,
        owner_tree,
        prefix_commit,
        metadata.commit_message,
        owner_environment,
    )
    return _run_git(
        project_root,
        [
            "format-patch",
            "--stdout",
            "--full-index",
            "--binary",
            f"--subject-prefix=PATCH {position}/{mailbox_count}",
            "-1",
            owner_commit,
        ],
    ).stdout


def _atomic_replace(path: Path, content: bytes) -> None:
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary_path = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary_path, path)
    finally:
        temporary_path.unlink(missing_ok=True)


def _renumber_mailbox_subject(
    path: Path,
    content: bytes,
    position: int,
    old_count: int,
    new_count: int,
) -> bytes:
    old = f"Subject: [PATCH {position}/{old_count}] ".encode()
    if content.count(old) != 1:
        raise DeltaWorkflowError(
            f"{path} must contain exactly one {old.decode()!r} header"
        )
    return content.replace(
        old,
        f"Subject: [PATCH {position}/{new_count}] ".encode(),
        1,
    )


def _finalize_new_delta_mailbox(
    worktree: Path,
    mailbox: str,
    expected_tree: str,
    replay: ReplayResult,
    project_root: Path,
) -> FinalizeResult:
    if PATCH_RE.fullmatch(mailbox) is None:
        raise DeltaWorkflowError(f"invalid new mailbox name: {mailbox!r}")
    if mailbox in replay.mailboxes:
        raise DeltaWorkflowError(f"new mailbox is already listed in series: {mailbox}")

    delta_root = project_root / "patches" / "delta"
    mailbox_path = delta_root / mailbox
    if mailbox_path.exists():
        raise DeltaWorkflowError(f"new mailbox path already exists: {mailbox_path}")
    new_number = int(mailbox[:4])
    if new_number <= max(int(entry[:4]) for entry in replay.mailboxes):
        raise DeltaWorkflowError("a new mailbox must append a higher logical slot")

    source_head, source_tree = _require_source_worktree(
        project_root,
        worktree,
        expected_tree,
        replay,
    )
    source_parents = _run_git(
        worktree,
        ["rev-list", "--parents", "-n", "1", source_head],
        cwd=worktree,
    ).stdout.split()
    if len(source_parents) != 2:
        raise DeltaWorkflowError("new mailbox source must contain exactly one WIP commit")
    source_parent_tree = _run_git(
        worktree,
        ["rev-parse", f"{source_parents[1]}^{{tree}}"],
        cwd=worktree,
    ).stdout.strip()
    if source_parent_tree != replay.tree:
        raise DeltaWorkflowError(
            "new mailbox source must be one WIP commit over the current queue"
        )

    old_count = len(replay.mailboxes)
    new_count = old_count + 1
    metadata = _read_commit_metadata(worktree, source_head)
    candidate = _candidate_mailbox(
        project_root,
        replay.tree,
        expected_tree,
        metadata,
        new_count,
        new_count,
    ).encode("utf-8")

    original_mailboxes: dict[Path, bytes] = {}
    renumbered_mailboxes: dict[Path, bytes] = {}
    for position, entry in enumerate(replay.mailboxes, start=1):
        path = delta_root / entry
        original = path.read_bytes()
        _read_mailbox_metadata(path, position, old_count)
        original_mailboxes[path] = original
        renumbered_mailboxes[path] = _renumber_mailbox_subject(
            path,
            original,
            position,
            old_count,
            new_count,
        )

    series_path = delta_root / "series"
    original_series = series_path.read_bytes()
    separator = b"" if original_series.endswith(b"\n") else b"\n"
    candidate_series = original_series + separator + mailbox.encode() + b"\n"

    with tempfile.TemporaryDirectory(prefix="herdr-delta-finalize-new-") as temp_dir:
        candidate_paths: list[Path] = []
        for path in original_mailboxes:
            candidate_path = Path(temp_dir) / path.name
            candidate_path.write_bytes(renumbered_mailboxes[path])
            candidate_paths.append(candidate_path)
        candidate_path = Path(temp_dir) / mailbox
        candidate_path.write_bytes(candidate)
        candidate_paths.append(candidate_path)
        candidate_tree = _tree_after_patches(
            project_root,
            replay.base,
            candidate_paths,
        )
    if candidate_tree != expected_tree:
        difference = _run_git(
            project_root,
            ["diff", "--stat", expected_tree, candidate_tree],
            check=False,
        ).stdout.strip()
        detail = f"\n{difference}" if difference else ""
        raise DeltaWorkflowError(
            "new mailbox does not reproduce the tested source tree: "
            f"expected {expected_tree}, found {candidate_tree}{detail}"
        )
    _run_git(project_root, ["diff", "--check", replay.base, candidate_tree])

    for path, original in original_mailboxes.items():
        if path.read_bytes() != original:
            raise DeltaWorkflowError(f"mailbox changed concurrently: {path.name}")
    if series_path.read_bytes() != original_series or mailbox_path.exists():
        raise DeltaWorkflowError("delta series changed concurrently")

    try:
        for path, content in renumbered_mailboxes.items():
            _atomic_replace(path, content)
        _atomic_replace(mailbox_path, candidate)
        _atomic_replace(series_path, candidate_series)
        verified = verify_replay_tree(expected_tree, project_root)
    except (DeltaWorkflowError, OSError) as error:
        _atomic_replace(series_path, original_series)
        for path, content in original_mailboxes.items():
            _atomic_replace(path, content)
        mailbox_path.unlink(missing_ok=True)
        raise DeltaWorkflowError(
            f"restored delta after new mailbox finalization failed: {error}"
        ) from error

    return FinalizeResult(
        mailbox=mailbox,
        source_head=source_head,
        source_tree=source_tree,
        replay_tree=verified.tree,
    )


def finalize_delta_mailbox(
    worktree: Path,
    mailbox: str,
    expected_tree: str,
    project_root: Path = PROJECT_ROOT,
    *,
    new_mailbox: bool = False,
) -> FinalizeResult:
    """Fold one tested WIP tree into one mailbox and prove exact queue replay."""

    project_root = project_root.resolve()
    _require_control_master(project_root)
    _require_clean_delta(project_root)
    _require_tree_object(project_root, expected_tree, "--expected-tree")

    replay = replay_delta_tree(project_root)
    if new_mailbox:
        return _finalize_new_delta_mailbox(
            worktree,
            mailbox,
            expected_tree,
            replay,
            project_root,
        )
    if mailbox not in replay.mailboxes:
        raise DeltaWorkflowError(f"--mailbox is not listed in series: {mailbox}")
    source_head, source_tree = _require_source_worktree(
        project_root,
        worktree,
        expected_tree,
        replay,
    )

    delta_root = project_root / "patches" / "delta"
    position = replay.mailboxes.index(mailbox) + 1
    mailbox_path = delta_root / mailbox
    original_mailbox = mailbox_path.read_bytes()
    metadata = _read_mailbox_metadata(
        mailbox_path,
        position,
        len(replay.mailboxes),
    )

    prefix_paths = [
        delta_root / entry for entry in replay.mailboxes[: position - 1]
    ]
    later_paths = [
        delta_root / entry for entry in reversed(replay.mailboxes[position:])
    ]
    prefix_tree = _tree_after_patches(project_root, replay.base, prefix_paths)
    owner_tree = _tree_after_patches(
        project_root,
        expected_tree,
        later_paths,
        reverse=True,
    )
    unchanged = _run_git(
        project_root,
        ["diff", "--quiet", prefix_tree, owner_tree],
        check=False,
    )
    if unchanged.returncode == 0:
        raise DeltaWorkflowError(f"finalized mailbox would be empty: {mailbox}")
    if unchanged.returncode not in (0, 1):
        raise DeltaWorkflowError("could not compare finalized mailbox trees")

    candidate = _candidate_mailbox(
        project_root,
        prefix_tree,
        owner_tree,
        metadata,
        position,
        len(replay.mailboxes),
    ).encode("utf-8")

    with tempfile.TemporaryDirectory(prefix="herdr-delta-finalize-") as temp_dir:
        candidate_path = Path(temp_dir) / mailbox
        candidate_path.write_bytes(candidate)
        candidate_paths = [
            candidate_path if entry == mailbox else delta_root / entry
            for entry in replay.mailboxes
        ]
        candidate_tree = _tree_after_patches(
            project_root,
            replay.base,
            candidate_paths,
        )
    if candidate_tree != expected_tree:
        difference = _run_git(
            project_root,
            ["diff", "--stat", expected_tree, candidate_tree],
            check=False,
        ).stdout.strip()
        detail = f"\n{difference}" if difference else ""
        raise DeltaWorkflowError(
            "finalized queue does not reproduce the tested source tree: "
            f"expected {expected_tree}, found {candidate_tree}{detail}"
        )
    _run_git(project_root, ["diff", "--check", replay.base, candidate_tree])

    if mailbox_path.read_bytes() != original_mailbox:
        raise DeltaWorkflowError(f"mailbox changed concurrently: {mailbox}")
    _atomic_replace(mailbox_path, candidate)
    try:
        verified = verify_replay_tree(expected_tree, project_root)
    except DeltaWorkflowError as error:
        _atomic_replace(mailbox_path, original_mailbox)
        raise DeltaWorkflowError(
            f"restored {mailbox} after final on-disk verification failed: {error}"
        ) from error

    return FinalizeResult(
        mailbox=mailbox,
        source_head=source_head,
        source_tree=source_tree,
        replay_tree=verified.tree,
    )


def materialize_delta_worktree(
    worktree: Path,
    project_root: Path = PROJECT_ROOT,
) -> WorktreeResult:
    """Replay the queue into an existing clean task worktree at recorded BASE."""

    project_root = project_root.resolve()
    _require_control_master(project_root)
    _require_clean_delta(project_root)
    base = _read_base(project_root)
    mailboxes = _read_series(project_root)
    worktree, branch = _require_clean_delta_worktree(project_root, worktree)

    initial_head = _run_git(
        worktree, ["rev-parse", "HEAD"], cwd=worktree
    ).stdout.strip()
    if initial_head != base:
        raise DeltaWorkflowError(
            "source worktree must start at the exact commit recorded in BASE: "
            f"expected {base}, found {initial_head}"
        )

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
                cwd=worktree,
                environment=replay_environment,
            )
        _run_git(worktree, ["diff", "--check", f"{base}..HEAD"], cwd=worktree)
    except DeltaWorkflowError as error:
        raise DeltaWorkflowError(
            f"delta replay stopped in {worktree}; preserve and inspect that worktree: {error}"
        ) from error

    head = _run_git(worktree, ["rev-parse", "HEAD"], cwd=worktree).stdout.strip()
    tree = _run_git(
        worktree, ["rev-parse", "HEAD^{tree}"], cwd=worktree
    ).stdout.strip()
    return WorktreeResult(
        branch=branch,
        path=worktree,
        base=base,
        head=head,
        tree=tree,
        mailbox_count=len(mailboxes),
    )


def publish_development_worktree(
    worktree: Path,
    project_root: Path = PROJECT_ROOT,
) -> DevelopmentResult:
    """Push the one clean cumulative development source state."""

    project_root = project_root.resolve()
    _require_control_master(project_root)
    _require_clean_delta(project_root)
    replay = replay_delta_tree(project_root)
    worktree, branch = _require_clean_delta_worktree(project_root, worktree)
    if branch != DEVELOPMENT_BRANCH:
        raise DeltaWorkflowError(
            f"development worktree must use {DEVELOPMENT_BRANCH!r}, found {branch!r}"
        )
    ancestor = _run_git(
        worktree,
        ["merge-base", "--is-ancestor", replay.base, "HEAD"],
        cwd=worktree,
        check=False,
    )
    if ancestor.returncode != 0:
        raise DeltaWorkflowError("recorded BASE is not an ancestor of development HEAD")
    reachable_trees = _run_git(
        worktree,
        ["log", "--format=%T", f"{replay.base}..HEAD"],
        cwd=worktree,
    ).stdout.splitlines()
    if replay.tree not in reachable_trees:
        raise DeltaWorkflowError(
            "development history does not contain the current checked-in replay tree"
        )
    head = _run_git(worktree, ["rev-parse", "HEAD"], cwd=worktree).stdout.strip()
    unintegrated = unintegrated_topic_worktrees(project_root, head)
    if unintegrated:
        detail = ", ".join(
            f"{topic.branch} ({topic.path})" for topic in unintegrated
        )
        raise DeltaWorkflowError(
            f"unintegrated topic worktrees block development publication: {detail}"
        )
    _run_git(worktree, ["diff", "--check", f"{replay.base}..HEAD"], cwd=worktree)
    _run_git(
        worktree,
        ["push", "origin", f"HEAD:{DEVELOPMENT_REMOTE_REF}"],
        cwd=worktree,
    )
    tree = _run_git(
        worktree, ["rev-parse", "HEAD^{tree}"], cwd=worktree
    ).stdout.strip()
    return DevelopmentResult(path=worktree, head=head, tree=tree)


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

    _require_control_master(project_root)
    _require_clean_delta(project_root)

    base = _read_base(project_root)
    branch = DEVELOPMENT_BRANCH if name == "development" else f"agent/delta-{name}"
    _run_git(project_root, ["check-ref-format", "--branch", branch])
    if branch == DEVELOPMENT_BRANCH:
        remote_exists = _run_git(
            project_root,
            ["ls-remote", "--exit-code", "--heads", "origin", DEVELOPMENT_REMOTE_REF],
            check=False,
        )
        if remote_exists.returncode == 0:
            raise DeltaWorkflowError(
                "remote cumulative development branch already exists; create the "
                "local development branch from its exact fetched tip"
            )
        if remote_exists.returncode != 2:
            detail = remote_exists.stderr.strip() or "could not inspect remote branch"
            raise DeltaWorkflowError(detail)
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
    return materialize_delta_worktree(path, project_root)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Materialize, finalize, and verify the herdr-win delta."
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

    materialize = commands.add_parser(
        "materialize",
        help="replay the queue into an existing clean task worktree at BASE",
    )
    materialize.add_argument(
        "--worktree",
        required=True,
        type=Path,
        help="absolute path to an existing delta worktree at BASE",
    )

    publish_development = commands.add_parser(
        "publish-development",
        help="push the clean cumulative development source state",
    )
    publish_development.add_argument(
        "--worktree",
        required=True,
        type=Path,
        help=f"absolute path to the shared {DEVELOPMENT_BRANCH} worktree",
    )

    commands.add_parser(
        "compile-prefixes",
        help="refresh-only replay and compile of every ordered mailbox prefix",
    )

    check = commands.add_parser(
        "check", help="replay into a temporary index without another checkout"
    )
    check.add_argument(
        "--expected-tree",
        help="exact tested Git tree ID that the checked-in queue must reproduce",
    )

    finalize = commands.add_parser(
        "finalize",
        help="fold one tested WIP tree into its owning mailbox",
    )
    finalize.add_argument(
        "--worktree",
        required=True,
        type=Path,
        help="absolute path to the clean replayed WIP worktree",
    )
    finalize.add_argument(
        "--mailbox",
        required=True,
        help="exact owning mailbox name from patches/delta/series",
    )
    finalize.add_argument(
        "--expected-tree",
        required=True,
        help="exact tested Git tree ID that final replay must reproduce",
    )
    finalize.add_argument(
        "--new-mailbox",
        action="store_true",
        help="append one higher-numbered mailbox from exactly one WIP commit",
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

        if options.command == "materialize":
            result = materialize_delta_worktree(options.worktree)
            print(f"worktree: {result.path}")
            print(f"branch: {result.branch}")
            print(f"base: {result.base}")
            print(f"head: {result.head}")
            print(f"tree: {result.tree}")
            print(f"mailboxes: {result.mailbox_count}")
            return 0

        if options.command == "finalize":
            result = finalize_delta_mailbox(
                options.worktree,
                options.mailbox,
                options.expected_tree,
                new_mailbox=options.new_mailbox,
            )
            print(f"mailbox: {result.mailbox}")
            print(f"source-head: {result.source_head}")
            print(f"source-tree: {result.source_tree}")
            print(f"replay-tree: {result.replay_tree}")
            print("mailbox-updated: yes")
            return 0

        if options.command == "publish-development":
            result = publish_development_worktree(options.worktree)
            print(f"worktree: {result.path}")
            print(f"head: {result.head}")
            print(f"tree: {result.tree}")
            print("development-pushed: yes")
            return 0

        if options.command == "compile-prefixes":
            mailboxes = compile_delta_prefixes()
            print(f"compiled-prefixes: {len(mailboxes)}")
            print(f"last-mailbox: {mailboxes[-1]}")
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
