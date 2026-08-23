#!/usr/bin/env python3
"""Generate the Homebrew mark-cli formula from release checksum files."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

REPOSITORY = "phongndo/mark"
FORMULA_TARGETS = (
    ("macos", "intel", "x86_64-apple-darwin"),
    ("macos", "arm", "aarch64-apple-darwin"),
    ("linux", "intel", "x86_64-unknown-linux-gnu"),
    ("linux", "arm", "aarch64-unknown-linux-gnu"),
)
TAG_PATTERN = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+(?:[.-][0-9A-Za-z.-]+)?$")
CHECKSUM_PATTERN = re.compile(r"^([0-9a-fA-F]{64})[ \t]+\*?([^\r\n]+)$")


def release_asset(tag: str, target: str) -> str:
    return f"mark-{tag}-{target}.tar.gz"


def read_checksum(checksum_dir: Path, asset: str) -> str:
    checksum_path = checksum_dir / f"{asset}.sha256"
    try:
        text = checksum_path.read_text(encoding="utf-8").strip()
    except FileNotFoundError as error:
        raise ValueError(f"missing checksum file: {checksum_path}") from error

    match = CHECKSUM_PATTERN.fullmatch(text)
    if match is None:
        raise ValueError(f"invalid checksum file: {checksum_path}")

    checksum, recorded_asset = match.groups()
    if recorded_asset != asset:
        raise ValueError(
            f"checksum file {checksum_path} names {recorded_asset!r}, expected {asset!r}"
        )
    return checksum.lower()


def render_formula(tag: str, checksum_dir: Path) -> str:
    if TAG_PATTERN.fullmatch(tag) is None:
        raise ValueError(f"invalid release tag: {tag}")

    version = tag.removeprefix("v")
    checksums = {
        target: read_checksum(checksum_dir, release_asset(tag, target))
        for _, _, target in FORMULA_TARGETS
    }

    lines = [
        "class MarkCli < Formula",
        '  desc "Fast, keyboard-first terminal Git diff reviewer"',
        f'  homepage "https://github.com/{REPOSITORY}"',
        f'  version "{version}"',
        '  license "MIT"',
        "",
    ]

    for platform in ("macos", "linux"):
        platform_targets = [
            (cpu, target)
            for target_platform, cpu, target in FORMULA_TARGETS
            if target_platform == platform
        ]
        lines.append(f"  on_{platform} do")
        for index, (cpu, target) in enumerate(platform_targets):
            asset = release_asset(tag, target)
            url = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{asset}"
            lines.extend(
                [
                    f"    on_{cpu} do",
                    f'      url "{url}"',
                    f'      sha256 "{checksums[target]}"',
                    "    end",
                ]
            )
            if index + 1 < len(platform_targets):
                lines.append("")
        lines.extend(["  end", ""])

    lines.extend(
        [
            '  conflicts_with "mark", because: "both install a `mark` executable"',
            "",
            "  def install",
            '    bin.install "mark"',
            "  end",
            "",
            "  test do",
            '    assert_match "mark #{version}", shell_output("#{bin}/mark --version")',
            "  end",
            "end",
            "",
        ]
    )
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True, help="release tag, for example v0.13.0")
    parser.add_argument(
        "--checksums",
        required=True,
        type=Path,
        help="directory containing release .sha256 files",
    )
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        formula = render_formula(args.tag, args.checksums)
    except ValueError as error:
        raise SystemExit(f"generate-homebrew-formula: {error}") from error

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(formula, encoding="utf-8")
    print(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
