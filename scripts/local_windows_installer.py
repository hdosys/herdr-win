#!/usr/bin/env python3
"""Build and reuse one validated local Windows installer candidate."""

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
import tempfile
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


PROJECT_ROOT = Path(__file__).resolve().parent.parent
TARGET_ROOT = PROJECT_ROOT / "target" / "x86_64-pc-windows-msvc"
INPUT_ROOT = TARGET_ROOT / "installer-inputs"
OUTPUT_PATH = TARGET_ROOT / "release" / "herdr-win_local_candidate_setup.exe"
FAULT_OUTPUT_ROOT = PROJECT_ROOT / "target" / "installer-faults"
NSIS_CACHE = TARGET_ROOT / "tools" / "nsis-3.12"
WINDOWS_TARGET = "x86_64-pc-windows-msvc"
DEFAULT_CARGO_TARGET = (
    Path(tempfile.gettempdir()) / "opencode" / "herdr-win-cargo-target"
)
BUILD_ID_RE = re.compile(r"^[0-9a-f]{12}\.[0-9a-f]{12}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
DYNAMIC_MSVC_RUNTIME_IMPORT = re.compile(
    r"(?im)^\s*((?:VCRUNTIME|MSVCP)[A-Z0-9_]*\.dll)\s*$"
)
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
    environment_overrides: dict[str, str] | None = None,
    removed_environment: Sequence[str] = (),
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["GIT_TERMINAL_PROMPT"] = "0"
    for name in removed_environment:
        environment.pop(name, None)
    if environment_overrides:
        environment.update(environment_overrides)
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


def _git_arguments(path: Path, arguments: Sequence[str]) -> list[str]:
    safe_directory = path.resolve().as_posix()
    return [
        "-c",
        "core.longpaths=true",
        "-c",
        f"safe.directory={safe_directory}",
        "-C",
        str(path),
        *arguments,
    ]


def _git(path: Path, arguments: Sequence[str]) -> str:
    return _run(
        "git",
        _git_arguments(path, arguments),
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
        "scripts/windows_installer_fault_test.ps1",
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


def _candidate_build_id(
    base_commit: str,
    source_commit: str,
    tracked_diff: str,
    untracked_files: Sequence[tuple[str, bytes]],
) -> str:
    if (
        COMMIT_RE.fullmatch(base_commit) is None
        or COMMIT_RE.fullmatch(source_commit) is None
    ):
        raise LocalInstallerError("candidate identity requires full lowercase commit IDs")
    digest = hashlib.sha256()
    digest.update(f"source-commit\0{source_commit}\0tracked-diff\0".encode())
    digest.update(tracked_diff.encode("utf-8"))
    for name, payload in sorted(untracked_files):
        digest.update(f"\0untracked\0{name}\0{len(payload)}\0".encode())
        digest.update(payload)
    return f"{base_commit[:12]}.{digest.hexdigest()[:12]}"


def _source_build_identity(source: Path) -> tuple[str, str]:
    base_path = _safe_path(
        PROJECT_ROOT / "patches" / "delta" / "BASE",
        "delta BASE",
        directory=False,
    )
    base_commit = base_path.read_text(encoding="utf-8").strip()
    source_commit = _git(source, ["rev-parse", "HEAD"])
    tracked_diff = _run(
        "git",
        _git_arguments(source, ["diff", "--binary", "--no-ext-diff", "HEAD"]),
        timeout=10,
    ).stdout
    untracked_output = _run(
        "git",
        _git_arguments(
            source, ["ls-files", "--others", "--exclude-standard", "-z"]
        ),
        timeout=10,
    ).stdout
    untracked_files: list[tuple[str, bytes]] = []
    for relative in filter(None, untracked_output.split("\0")):
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts:
            raise LocalInstallerError(
                f"untracked source path escapes the worktree: {relative}"
            )
        source_file = _safe_path(
            source / path, f"untracked source {relative}", directory=False
        )
        untracked_files.append((path.as_posix(), source_file.read_bytes()))
    return (
        _candidate_build_id(base_commit, source_commit, tracked_diff, untracked_files),
        base_commit,
    )


def _cargo_build_arguments(cargo_target: Path, jobs: int) -> list[str]:
    if jobs < 1:
        raise LocalInstallerError("Cargo requires at least one build job")
    return [
        "build",
        "--release",
        "--locked",
        "--target",
        WINDOWS_TARGET,
        "--target-dir",
        str(cargo_target),
        "--jobs",
        str(jobs),
        "--bin",
        "herdr",
        "--bin",
        "herdr-launcher",
        "--bin",
        "herdr-installer-helper",
    ]


def _dynamic_msvc_runtime_imports(dependencies: str) -> list[str]:
    return sorted(
        {
            match.group(1).upper()
            for match in DYNAMIC_MSVC_RUNTIME_IMPORT.finditer(dependencies)
        }
    )


def _verify_self_contained_windows_executables(paths: Sequence[Path]) -> None:
    for path in paths:
        dependencies = _run(
            "dumpbin.exe",
            ["/DEPENDENTS", str(path)],
            timeout=30,
        ).stdout
        imports = _dynamic_msvc_runtime_imports(dependencies)
        if imports:
            raise LocalInstallerError(
                f"{path.name} requires a non-inbox MSVC runtime: {', '.join(imports)}"
            )


def _available_cpu_count() -> int:
    process_cpu_count = getattr(os, "process_cpu_count", os.cpu_count)
    return process_cpu_count() or 1


def _directory(path: Path, label: str) -> Path:
    if not path.is_absolute():
        raise LocalInstallerError(f"{label} must be an absolute path")
    path.mkdir(parents=True, exist_ok=True)
    return _safe_path(path, label, directory=True)


def _windows_powershell() -> Path:
    return _safe_path(
        Path(os.environ["SystemRoot"])
        / "System32/WindowsPowerShell/v1.0/powershell.exe",
        "Windows PowerShell 5.1",
        directory=False,
    )


def _conpty_package_path(source: Path) -> Path:
    metadata = _safe_path(
        source / "packaging" / "windows" / "conpty.json",
        "source ConPTY metadata",
        directory=False,
    )
    try:
        package = json.loads(metadata.read_text(encoding="utf-8"))["package"]
        package_id = str(package["id"])
        version = str(package["version"])
    except (OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError) as error:
        raise LocalInstallerError(
            f"could not read source ConPTY package identity: {error}"
        ) from error
    if re.fullmatch(r"[A-Za-z0-9.]+", package_id) is None or re.fullmatch(
        r"[0-9]+(?:\.[0-9]+)+", version
    ) is None:
        raise LocalInstallerError("source ConPTY package identity is invalid")
    return (
        PROJECT_ROOT
        / "target"
        / "package-cache"
        / f"{package_id}.{version}.nupkg"
    )


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
    runtime = _safe_path(stage / "herdr.exe", "bundle runtime", directory=False)
    launcher = _safe_path(
        bundle / "herdr-launcher.exe", "bundle launcher", directory=False
    )
    helper = _safe_path(
        bundle / "herdr-installer-helper.exe", "bundle helper", directory=False
    )
    _verify_self_contained_windows_executables((runtime, launcher, helper))
    _validate_stage(source, stage)
    if _identity(stage, launcher) != recorded_identity:
        raise LocalInstallerError("bundle manifest does not match executable identities")
    return recorded_identity


def prepare(options: argparse.Namespace) -> None:
    source = _source_root(options.source_worktree)
    stage = _safe_path(options.stage_dir, "--stage-dir", directory=True)
    launcher = _safe_path(options.launcher_exe, "--launcher-exe", directory=False)
    helper = _safe_path(options.installer_helper_exe, "--installer-helper-exe", directory=False)
    _verify_self_contained_windows_executables(
        (
            _safe_path(stage / "herdr.exe", "staged runtime", directory=False),
            launcher,
            helper,
        )
    )
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
    powershell = _windows_powershell()
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
        print(
            result.stderr,
            file=sys.stderr,
            end="" if result.stderr.endswith("\n") else "\n",
        )
    print(f"elapsed_seconds={time.monotonic() - started:.3f}")


def release_precheck(options: argparse.Namespace) -> None:
    source = _source_root(options.source_worktree)
    bundle = _safe_path(options.input_bundle, "--input-bundle", directory=True)
    identity = validate_bundle(source, bundle)
    output_root = _directory(FAULT_OUTPUT_ROOT, "installer fault output root")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="run-", dir=output_root) as temporary:
        result = _run(
            _windows_powershell(),
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(source / "scripts/windows_installer_fault_test.ps1"),
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
                "-OutputDir",
                temporary,
            ],
            cwd=source,
            timeout=1800,
        )
    if result.stdout:
        print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    if result.stderr:
        print(result.stderr, file=sys.stderr, end="" if result.stderr.endswith("\n") else "\n")
    print(f"elapsed_seconds={time.monotonic() - started:.3f}")


def candidate(options: argparse.Namespace) -> None:
    total_started = time.monotonic()
    source = _source_root(options.source_worktree)
    cargo_target = _directory(options.cargo_target_dir, "--cargo-target-dir")
    build_id, base_commit = _source_build_identity(source)
    jobs = _available_cpu_count()
    print(f"build_id={build_id}")
    print(f"cargo_jobs={jobs}")
    print(f"cargo_target={cargo_target}")

    cargo_started = time.monotonic()
    cargo_result = _run(
        "cargo",
        _cargo_build_arguments(cargo_target, jobs),
        cwd=source,
        timeout=1200,
        environment_overrides={
            "HERDR_BUILD_ID": build_id,
            "HERDR_BUILD_COMMIT": base_commit,
        },
        removed_environment=("HERDR_RELEASE_VERSION",),
    )
    if cargo_result.stdout:
        print(cargo_result.stdout, end="" if cargo_result.stdout.endswith("\n") else "\n")
    if cargo_result.stderr:
        print(
            cargo_result.stderr,
            file=sys.stderr,
            end="" if cargo_result.stderr.endswith("\n") else "\n",
        )
    print(f"cargo_elapsed_seconds={time.monotonic() - cargo_started:.3f}")

    release = cargo_target / WINDOWS_TARGET / "release"
    runtime = _safe_path(release / "herdr.exe", "candidate runtime", directory=False)
    launcher = _safe_path(
        release / "herdr-launcher.exe", "candidate launcher", directory=False
    )
    helper = _safe_path(
        release / "herdr-installer-helper.exe",
        "candidate installer helper",
        directory=False,
    )
    temporary_parent = _directory(TARGET_ROOT / "tmp", "candidate temporary root")
    with tempfile.TemporaryDirectory(
        prefix=f"candidate-{build_id}-", dir=temporary_parent
    ) as temporary:
        stage = Path(temporary) / "stage"
        _run(
            sys.executable,
            [
                str(source / "scripts" / "package_windows_conpty.py"),
                "stage",
                "--package",
                str(_conpty_package_path(source)),
                "--herdr-exe",
                str(runtime),
                "--output-dir",
                str(stage),
            ],
            cwd=source,
            timeout=180,
        )
        prepare(
            argparse.Namespace(
                source_worktree=source,
                stage_dir=stage,
                launcher_exe=launcher,
                installer_helper_exe=helper,
            )
        )

    build(
        argparse.Namespace(
            source_worktree=source,
            input_bundle=INPUT_ROOT / build_id,
        )
    )
    print(f"total_elapsed_seconds={time.monotonic() - total_started:.3f}")


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
    precheck_command = commands.add_parser("release-precheck")
    precheck_command.add_argument("--source-worktree", required=True, type=Path)
    precheck_command.add_argument("--input-bundle", required=True, type=Path)
    candidate_command = commands.add_parser("candidate")
    candidate_command.add_argument("--source-worktree", required=True, type=Path)
    candidate_command.add_argument(
        "--cargo-target-dir", type=Path, default=DEFAULT_CARGO_TARGET
    )
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    options = _parser().parse_args(arguments)
    try:
        {
            "prepare": prepare,
            "build": build,
            "release-precheck": release_precheck,
            "candidate": candidate,
        }[options.command](options)
        return 0
    except LocalInstallerError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
