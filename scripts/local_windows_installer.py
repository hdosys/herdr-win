#!/usr/bin/env python3
"""Prepare and reuse one validated local Windows installer input bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


PROJECT_ROOT = Path(__file__).resolve().parent.parent
TARGET_ROOT = PROJECT_ROOT / "target" / "x86_64-pc-windows-msvc"
INPUT_ROOT = TARGET_ROOT / "installer-inputs"
OUTPUT_PATH = TARGET_ROOT / "release" / "herdr-win_local_candidate_setup.exe"
NSIS_CACHE = TARGET_ROOT / "tools" / "nsis-3.12"
BUILD_ID_RE = re.compile(r"^[0-9a-f]{12}\.[0-9a-f]{12}$")
LOCAL_VERSION_RE = re.compile(
    r"^herdr-win local \(Herdr (?P<base>[0-9]+\.[0-9]+\.[0-9]+), "
    r"build (?P<build>[0-9a-f]{12}\.[0-9a-f]{12})\)$"
)
REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)


class LocalInstallerError(RuntimeError):
    pass


@dataclass(frozen=True)
class InstallerIdentity:
    build_id: str
    base_version: str


def _run(
    command: Path | str,
    arguments: Sequence[str],
    *,
    cwd: Path | None = None,
    timeout: int,
    clean_runtime_environment: bool = False,
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["GIT_TERMINAL_PROMPT"] = "0"
    if clean_runtime_environment:
        environment.pop("HERDR_REMOTE_SIDECAR_V1", None)
    try:
        result = subprocess.run(
            [str(command), *arguments],
            cwd=cwd,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise LocalInstallerError(f"could not run {command!s}: {error}") from error
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise LocalInstallerError(
            f"{command!s} failed with exit code {result.returncode}: {detail}"
        )
    return result


def _safe_path(path: Path, label: str, *, directory: bool) -> Path:
    if not path.is_absolute():
        raise LocalInstallerError(f"{label} must be an absolute path")
    try:
        original_status = path.lstat()
        if path.is_symlink() or bool(
            getattr(original_status, "st_file_attributes", 0) & REPARSE_POINT
        ):
            raise LocalInstallerError(f"{label} must not be a reparse path: {path}")
        resolved = path.resolve(strict=True)
        status = resolved.lstat()
    except LocalInstallerError:
        raise
    except OSError as error:
        raise LocalInstallerError(f"could not resolve {label} {path}: {error}") from error
    expected = stat.S_ISDIR(status.st_mode) if directory else stat.S_ISREG(status.st_mode)
    reparse = bool(getattr(status, "st_file_attributes", 0) & REPARSE_POINT)
    if not expected or reparse or (not directory and status.st_size <= 0):
        kind = "directory" if directory else "nonempty file"
        raise LocalInstallerError(f"{label} must be a regular non-reparse {kind}: {resolved}")
    return resolved


def _git(path: Path, arguments: Sequence[str]) -> str:
    return _run(
        "git",
        ["-c", "core.longpaths=true", "-C", str(path), *arguments],
        timeout=10,
    ).stdout.strip()


def _source_root(path: Path) -> Path:
    source = _safe_path(path, "--source-worktree", directory=True)
    if Path(_git(source, ["rev-parse", "--show-toplevel"])).resolve() != source:
        raise LocalInstallerError("--source-worktree must identify its Git root")
    control_common = Path(
        _git(PROJECT_ROOT, ["rev-parse", "--path-format=absolute", "--git-common-dir"])
    ).resolve()
    source_common = Path(
        _git(source, ["rev-parse", "--path-format=absolute", "--git-common-dir"])
    ).resolve()
    if os.path.normcase(control_common) != os.path.normcase(source_common):
        raise LocalInstallerError("source worktree belongs to another Git repository")
    branch = _git(source, ["symbolic-ref", "--short", "HEAD"])
    if not branch.startswith("agent/delta-"):
        raise LocalInstallerError(f"source worktree uses unsupported branch {branch!r}")
    for relative in (
        "scripts/package_windows_conpty.py",
        "scripts/package_windows_installer.ps1",
    ):
        _safe_path(source / relative, f"source {relative}", directory=False)
    return source


def parse_identity(version: str, launcher_build_id: str) -> InstallerIdentity:
    if BUILD_ID_RE.fullmatch(launcher_build_id) is None:
        raise LocalInstallerError(f"launcher returned invalid build ID {launcher_build_id!r}")
    match = LOCAL_VERSION_RE.fullmatch(version)
    if match is None:
        raise LocalInstallerError(f"runtime is not an exact local build: {version!r}")
    if match.group("build") != launcher_build_id:
        raise LocalInstallerError("runtime and launcher report different local build identities")
    return InstallerIdentity(launcher_build_id, match.group("base"))


def _identity(stage: Path, launcher: Path) -> InstallerIdentity:
    runtime = _safe_path(stage / "herdr.exe", "bundle runtime", directory=False)
    launcher = _safe_path(launcher, "bundle launcher", directory=False)
    build_id = _run(
        launcher,
        ["--herdr-private-launcher-build-id-v1"],
        timeout=10,
    ).stdout.strip()
    version = _run(
        runtime,
        ["--version"],
        timeout=10,
        clean_runtime_environment=True,
    ).stdout.strip()
    return parse_identity(version, build_id)


def _files(root: Path) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for current, directories, names in os.walk(root, followlinks=False):
        current_path = Path(current)
        for name in directories:
            _safe_path(current_path / name, "bundle entry", directory=True)
        for name in names:
            path = _safe_path(current_path / name, "bundle entry", directory=False)
            result[path.relative_to(root).as_posix()] = path
    return result


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _hashes(root: Path) -> dict[str, str]:
    return {
        name: _sha256(path)
        for name, path in sorted(_files(root).items())
        if name != "bundle.json"
    }


def _validate_stage(source: Path, stage: Path) -> None:
    _run(
        sys.executable,
        [
            str(source / "scripts" / "package_windows_conpty.py"),
            "validate",
            "--stage-dir",
            str(stage),
        ],
        cwd=source,
        timeout=60,
    )


def _bundle_manifest(bundle: Path) -> tuple[InstallerIdentity, dict[str, str]]:
    manifest = _safe_path(bundle / "bundle.json", "bundle manifest", directory=False)
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise LocalInstallerError(f"could not read bundle manifest: {error}") from error
    if set(data) != {"schema", "build_id", "base_version", "files"}:
        raise LocalInstallerError("bundle manifest has unsupported fields")
    if data["schema"] != 1 or not isinstance(data["files"], dict):
        raise LocalInstallerError("bundle manifest has unsupported schema")
    identity = InstallerIdentity(str(data["build_id"]), str(data["base_version"]))
    hashes = {str(name): str(value) for name, value in data["files"].items()}
    if BUILD_ID_RE.fullmatch(identity.build_id) is None or any(
        re.fullmatch(r"[0-9a-f]{64}", value) is None for value in hashes.values()
    ):
        raise LocalInstallerError("bundle manifest contains invalid identity or hash")
    return identity, hashes


def validate_bundle(source: Path, path: Path) -> InstallerIdentity:
    bundle = _safe_path(path, "--input-bundle", directory=True)
    try:
        bundle.relative_to(INPUT_ROOT.resolve())
    except ValueError as error:
        raise LocalInstallerError(f"input bundle must remain below {INPUT_ROOT}") from error
    recorded_identity, recorded_hashes = _bundle_manifest(bundle)
    if _hashes(bundle) != recorded_hashes:
        raise LocalInstallerError("installer input bundle files or hashes changed")
    stage = _safe_path(bundle / "stage", "bundle stage", directory=True)
    launcher = bundle / "herdr-launcher.exe"
    _safe_path(bundle / "herdr-installer-helper.exe", "bundle helper", directory=False)
    _validate_stage(source, stage)
    if _identity(stage, launcher) != recorded_identity:
        raise LocalInstallerError("bundle manifest does not match executable identities")
    return recorded_identity


def prepare(options: argparse.Namespace) -> None:
    source = _source_root(options.source_worktree)
    stage = _safe_path(options.stage_dir, "--stage-dir", directory=True)
    launcher = _safe_path(options.launcher_exe, "--launcher-exe", directory=False)
    helper = _safe_path(options.installer_helper_exe, "--installer-helper-exe", directory=False)
    _validate_stage(source, stage)
    identity = _identity(stage, launcher)
    INPUT_ROOT.mkdir(parents=True, exist_ok=True)
    destination = INPUT_ROOT / identity.build_id
    if destination.exists():
        if validate_bundle(source, destination) != identity:
            raise LocalInstallerError("existing bundle uses another identity")
        print(f"bundle={destination}")
        print("reused=yes")
        return

    temporary = INPUT_ROOT / f".{identity.build_id}.prepare-{uuid.uuid4().hex}"
    temporary.mkdir()
    try:
        (temporary / "stage").mkdir()
        for relative, path in _files(stage).items():
            target = temporary / "stage" / Path(relative)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
        shutil.copy2(launcher, temporary / "herdr-launcher.exe")
        shutil.copy2(helper, temporary / "herdr-installer-helper.exe")
        manifest = {
            "schema": 1,
            "build_id": identity.build_id,
            "base_version": identity.base_version,
            "files": _hashes(temporary),
        }
        (temporary / "bundle.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        validate_bundle(source, temporary)
        temporary.rename(destination)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)
    print(f"bundle={destination}")
    print("reused=no")


def build(options: argparse.Namespace) -> None:
    source = _source_root(options.source_worktree)
    bundle = _safe_path(options.input_bundle, "--input-bundle", directory=True)
    identity = validate_bundle(source, bundle)
    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    powershell = _safe_path(
        Path(os.environ["SystemRoot"])
        / "System32/WindowsPowerShell/v1.0/powershell.exe",
        "Windows PowerShell 5.1",
        directory=False,
    )
    started = time.monotonic()
    result = _run(
        powershell,
        [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(source / "scripts/package_windows_installer.ps1"),
            "-StageDir",
            str(bundle / "stage"),
            "-LauncherExe",
            str(bundle / "herdr-launcher.exe"),
            "-InstallerHelperExe",
            str(bundle / "herdr-installer-helper.exe"),
            "-BuildId",
            identity.build_id,
            "-ReleaseVersion",
            "local",
            "-BaseVersion",
            identity.base_version,
            "-OutputPath",
            str(OUTPUT_PATH),
            "-NsisCacheDir",
            str(NSIS_CACHE),
        ],
        cwd=source,
        timeout=300,
    )
    if result.stdout:
        print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="" if result.stderr.endswith("\n") else "\n")
    print(f"elapsed_seconds={time.monotonic() - started:.3f}")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_command = commands.add_parser("prepare")
    prepare_command.add_argument("--source-worktree", required=True, type=Path)
    prepare_command.add_argument("--stage-dir", required=True, type=Path)
    prepare_command.add_argument("--launcher-exe", required=True, type=Path)
    prepare_command.add_argument("--installer-helper-exe", required=True, type=Path)
    build_command = commands.add_parser("build")
    build_command.add_argument("--source-worktree", required=True, type=Path)
    build_command.add_argument("--input-bundle", required=True, type=Path)
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    options = _parser().parse_args(arguments)
    try:
        (prepare if options.command == "prepare" else build)(options)
        return 0
    except LocalInstallerError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
