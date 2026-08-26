#!/usr/bin/env python3
"""Render-check a changed staged README Mermaid block without keeping output."""

from __future__ import annotations

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Sequence


PROJECT_ROOT = Path(__file__).resolve().parent.parent
README_PATHS = ("README.md", "docs/next/README.md")
MERMAID_BLOCK = re.compile(r"```mermaid\s*\n(?P<source>.*?)\n```", re.DOTALL)
MIN_ASPECT_RATIO = 0.75
MAX_ASPECT_RATIO = 2.0


class MermaidCheckError(RuntimeError):
    pass


def extract_mermaid(markdown: str) -> str:
    matches = list(MERMAID_BLOCK.finditer(markdown))
    if len(matches) != 1:
        raise MermaidCheckError(
            f"README must contain exactly one Mermaid block, found {len(matches)}"
        )
    return matches[0].group("source").strip() + "\n"


def svg_dimensions(svg: Path) -> tuple[float, float]:
    try:
        root = ET.parse(svg).getroot()
    except (OSError, ET.ParseError) as error:
        raise MermaidCheckError(f"could not parse rendered Mermaid SVG: {error}") from error
    view_box = root.attrib.get("viewBox", "").split()
    if len(view_box) != 4:
        raise MermaidCheckError("rendered Mermaid SVG has no four-value viewBox")
    try:
        width = float(view_box[2])
        height = float(view_box[3])
    except ValueError as error:
        raise MermaidCheckError("rendered Mermaid SVG has a nonnumeric viewBox") from error
    if width <= 0 or height <= 0:
        raise MermaidCheckError("rendered Mermaid SVG dimensions must be positive")
    return width, height


def require_balanced_aspect(width: float, height: float) -> float:
    ratio = width / height
    if not MIN_ASPECT_RATIO <= ratio <= MAX_ASPECT_RATIO:
        raise MermaidCheckError(
            "rendered Mermaid diagram has an unreadable README aspect ratio: "
            f"{width:g}x{height:g} ({ratio:.3f})"
        )
    return ratio


def render_mermaid(source: str, command: Sequence[str]) -> tuple[float, float, float]:
    if not command:
        raise MermaidCheckError("Mermaid renderer command must not be empty")
    with tempfile.TemporaryDirectory(prefix="herdr-readme-mermaid-") as temporary:
        root = Path(temporary)
        input_path = root / "diagram.mmd"
        output_path = root / "diagram.svg"
        input_path.write_text(source, encoding="utf-8", newline="\n")
        try:
            result = subprocess.run(
                [*command, "-i", str(input_path), "-o", str(output_path), "-b", "transparent"],
                cwd=PROJECT_ROOT,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                check=False,
                timeout=120,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise MermaidCheckError(f"could not run Mermaid renderer: {error}") from error
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
            raise MermaidCheckError(f"Mermaid renderer failed: {detail}")
        width, height = svg_dimensions(output_path)
        return width, height, require_balanced_aspect(width, height)


def _git(arguments: Sequence[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    command = [
        "git",
        "-c",
        f"safe.directory={PROJECT_ROOT.resolve().as_posix()}",
        "-C",
        str(PROJECT_ROOT),
        *arguments,
    ]
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=30,
    )
    if check and result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise MermaidCheckError(f"{' '.join(command)!r} failed: {detail}")
    return result


def staged_mermaid_change() -> str | None:
    changed = set(
        _git(["diff", "--cached", "--name-only", "--", *README_PATHS])
        .stdout.splitlines()
    )
    if not changed:
        return None
    staged = [extract_mermaid(_git(["show", f":{path}"]).stdout) for path in README_PATHS]
    if staged[0] != staged[1]:
        raise MermaidCheckError("staged README Mermaid blocks are not mirrored")
    previous = [
        _git(["show", f"HEAD:{path}"], check=False) for path in README_PATHS
    ]
    if all(result.returncode == 0 and extract_mermaid(result.stdout) == staged[0] for result in previous):
        return None
    return staged[0]


def _renderer_command() -> list[str]:
    configured = os.environ.get("MERMAID_CLI", "mmdc")
    command = shlex.split(configured, posix=os.name != "nt")
    if os.name == "nt":
        command = [
            value[1:-1]
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'"
            else value
            for value in command
        ]
    if command:
        resolved = shutil.which(command[0])
        if resolved is not None:
            command[0] = resolved
    return command


def main(arguments: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--staged", action="store_true", help="check only a staged Mermaid change")
    options = parser.parse_args(arguments)
    try:
        source = staged_mermaid_change() if options.staged else extract_mermaid(
            (PROJECT_ROOT / "README.md").read_text(encoding="utf-8")
        )
        if source is None:
            print("README Mermaid source unchanged")
            return 0
        width, height, ratio = render_mermaid(source, _renderer_command())
        print(f"README Mermaid render: {width:g}x{height:g}, aspect {ratio:.3f}")
        return 0
    except (OSError, UnicodeError, MermaidCheckError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
