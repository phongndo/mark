#!/usr/bin/env python3
"""Build deterministic Markdown and repeated core TextMate benchmark corpora."""

import hashlib
import json
import re
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target" / "textmate-performance" / "corpora"
MARKDOWN_SOURCES = [
    "README.md",
    "CONTRIBUTING.md",
    "docs/architecture.md",
    "docs/configuration.md",
    "docs/development.md",
    "docs/usage.md",
    "pi-mark/README.md",
]


def tokenizer_lines(data: bytes) -> int:
    return data.count(b"\n") + 1


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build_markdown() -> dict:
    chunks = []
    for relative in MARKDOWN_SOURCES:
        data = (ROOT / relative).read_bytes()
        chunks.append(f"<!-- corpus-source: {relative} -->\n".encode())
        chunks.append(data)
        if not data.endswith(b"\n"):
            chunks.append(b"\n")
    data = b"".join(chunks)
    path = OUT / "representative-markdown.md"
    path.write_bytes(data)
    return {
        "id": "representative-markdown",
        "path": str(path.relative_to(ROOT)),
        "language": "markdown",
        "scope": "text.html.markdown",
        "bytes": len(data),
        "tokenizer_lines": tokenizer_lines(data),
        "sha256": digest(data),
        "sources": MARKDOWN_SOURCES,
    }


def build_core() -> dict:
    cases_path = ROOT / "crates/mark-syntax/tests/fixtures/textmate/cases.toml"
    cases = []
    for block in re.split(r"(?m)^\[\[case\]\]\s*$", cases_path.read_text())[1:]:
        language = re.search(r'(?m)^language\s*=\s*"([^"]+)"', block)
        fixture = re.search(r'(?m)^fixture\s*=\s*"([^"]+)"', block)
        if language and fixture:
            cases.append({"language": language.group(1), "fixture": fixture.group(1)})
    largest = {}
    for case in cases:
        path = ROOT / case["fixture"]
        size = path.stat().st_size
        if case["language"] not in largest or size > largest[case["language"]][0]:
            largest[case["language"]] = (size, path)

    target_per_language = 3_240_000 // len(largest)
    output_dir = OUT / "core-repeated"
    if output_dir.exists():
        shutil.rmtree(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    files = []
    aggregate = hashlib.sha256()
    generated_aggregate = hashlib.sha256()
    total_bytes = 0
    total_lines = 0
    for language, (_, source) in sorted(largest.items()):
        seed = source.read_bytes()
        if not seed.endswith(b"\n"):
            seed += b"\n"
        repeats = max(1, (target_per_language + len(seed) - 1) // len(seed))
        data = seed * repeats
        language_dir = output_dir / language
        language_dir.mkdir(parents=True, exist_ok=True)
        path = language_dir / source.name
        path.write_bytes(data)
        relative = str(path.relative_to(ROOT))
        generated_aggregate.update(relative.encode())
        generated_aggregate.update(b"\0")
        generated_aggregate.update(data)
        # The checked-in Make smoke fixture uses a .mk basename that Mark does
        # not detect. Keep generating it for coverage, but preserve the
        # canonical 29-detected-language acceptance corpus.
        detected = language != "make"
        if detected:
            aggregate.update(relative.encode())
            aggregate.update(b"\0")
            aggregate.update(data)
            total_bytes += len(data)
            total_lines += tokenizer_lines(data)
        files.append(
            {
                "language": language,
                "path": relative,
                "bytes": len(data),
                "tokenizer_lines": tokenizer_lines(data),
                "sha256": digest(data),
                "detected": detected,
            }
        )
    return {
        "id": "core-repeated",
        "languages": sum(file["detected"] for file in files),
        "generated_languages": len(files),
        "bytes": total_bytes,
        "tokenizer_lines": total_lines,
        "sha256": aggregate.hexdigest(),
        "generated_sha256": generated_aggregate.hexdigest(),
        "files": files,
    }


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    manifest = {"version": 1, "corpora": [build_markdown(), build_core()]}
    path = OUT / "manifest.json"
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"manifest": str(path.relative_to(ROOT)), **manifest}, indent=2))


if __name__ == "__main__":
    main()
