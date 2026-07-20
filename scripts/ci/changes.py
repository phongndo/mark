#!/usr/bin/env python3
"""Classify a Git diff into the CI lanes that it can affect."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from typing import Iterable

LANES = ("rust", "syntax", "generated", "performance", "pi", "workflows")
ZERO_SHA = "0" * 40
GENERATED_DOC_SOURCES = {
    "README.md",
    "docs/configuration.md",
    "docs/textmate-engine.md",
}


def _is(path: str, *names: str) -> bool:
    return path in names


def classify_paths(paths: Iterable[str]) -> dict[str, bool]:
    result = {lane: False for lane in LANES}

    for raw_path in paths:
        path = raw_path.removeprefix("./")

        # Changes to orchestration or the classifier itself exercise every lane;
        # otherwise a broken conditional job could validate only actionlint.
        if (
            path.startswith(".github/workflows/")
            or path.startswith(".github/actions/")
            or path.startswith("scripts/ci/")
        ):
            return {lane: True for lane in LANES}

        if path.startswith("pi-mark/"):
            result["pi"] = True

        if path.startswith("crates/"):
            result["rust"] = True
            result["performance"] = True
            if path.startswith("crates/mark-syntax/"):
                result["syntax"] = True
                result["generated"] = True

        if path.startswith("assets/"):
            result["syntax"] = True
            result["generated"] = True
            result["performance"] = True

        if path.startswith("benchmarks/"):
            result["generated"] = True
            result["performance"] = True

        if path.startswith("tools/"):
            result["generated"] = True
            if "bench" in path or "profile" in path:
                result["performance"] = True

        if path.startswith("scripts/"):
            result["rust"] = True
            if _is(path, "scripts/check-startup", "scripts/build-pgo"):
                result["performance"] = True

        if path == "docs/language-status.md" or path in GENERATED_DOC_SOURCES:
            result["generated"] = True

        if _is(path, "Cargo.toml", "Cargo.lock", "rust-toolchain.toml") or (
            path.startswith("crates/") and path.endswith("/Cargo.toml")
        ):
            result["rust"] = True
            result["syntax"] = True
            result["generated"] = True
            result["performance"] = True

        if _is(path, "flake.nix", "flake.lock", "mise.toml", "hk.pkl", "justfile"):
            result["rust"] = True
            result["workflows"] = True

    return result


def changed_paths(base: str | None, head: str) -> list[str] | None:
    if not base or base == ZERO_SHA:
        return None

    for revision in (base, head):
        check = subprocess.run(
            ["git", "cat-file", "-e", f"{revision}^{{commit}}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if check.returncode != 0:
            return None

    command = [
        "git",
        "diff",
        "--name-only",
        "--diff-filter=ACDMRTUXB",
        f"{base}...{head}",
    ]
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        print(completed.stderr, file=sys.stderr, end="")
        return None
    return [line for line in completed.stdout.splitlines() if line]


def write_outputs(result: dict[str, bool], output_path: str | None) -> None:
    lines = [f"{lane}={'true' if result[lane] else 'false'}" for lane in LANES]
    if output_path:
        with open(output_path, "a", encoding="utf-8") as output:
            output.write("\n".join(lines) + "\n")
    else:
        print("\n".join(lines))


def write_summary(paths: list[str] | None, result: dict[str, bool]) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    selected = ", ".join(lane for lane in LANES if result[lane]) or "none (documentation-only)"
    path_summary = "full validation fallback" if paths is None else f"{len(paths)} changed path(s)"
    with open(summary_path, "a", encoding="utf-8") as summary:
        summary.write("## CI impact\n\n")
        summary.write(f"- Diff: {path_summary}\n")
        summary.write(f"- Selected lanes: {selected}\n")
        if paths:
            summary.write("\n<details><summary>Changed paths</summary>\n\n```text\n")
            summary.write("\n".join(paths))
            summary.write("\n```\n</details>\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default=os.environ.get("BASE_SHA"))
    parser.add_argument("--head", default=os.environ.get("HEAD_SHA", "HEAD"))
    parser.add_argument("--output", default=os.environ.get("GITHUB_OUTPUT"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    paths = changed_paths(args.base, args.head)
    if paths is not None:
        subprocess.run(
            [
                "git",
                "diff",
                "--check",
                f"{args.base}...{args.head}",
                "--",
                ".",
                ":(exclude)crates/mark-syntax/tests/fixtures/**",
            ],
            check=True,
        )
    result = {lane: True for lane in LANES} if paths is None else classify_paths(paths)

    if paths is None:
        print("Unable to establish a complete base diff; selecting every CI lane.")
    else:
        print(f"Classified {len(paths)} changed path(s).")
        for path in paths:
            print(f"  {path}")

    write_outputs(result, args.output)
    write_summary(paths, result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
