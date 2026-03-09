#!/usr/bin/env python3
"""Search the Nexum Graph specification and whitepaper .docx files."""

from __future__ import annotations

import argparse
import sys
import textwrap
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

NS = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}

DOCUMENTS = {
    "spec": "Project_Codex_Final_Implementation_Spec.docx",
    "whitepaper-v1": "Project_Codex_Whitepaper_v1.docx",
    "whitepaper-v3": "Project_Codex_Whitepaper_v3.docx",
}


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def cache_path(doc_key: str) -> Path:
    return repo_root() / ".nex" / "cache" / "spec-text" / f"{doc_key}.txt"


def extract_text(doc_key: str, refresh: bool) -> list[str]:
    cache_file = cache_path(doc_key)
    if cache_file.exists() and not refresh:
        return cache_file.read_text(encoding="utf-8").splitlines()

    doc_path = repo_root() / DOCUMENTS[doc_key]
    if not doc_path.exists():
        raise FileNotFoundError(f"missing document: {doc_path}")

    with zipfile.ZipFile(doc_path) as archive:
        xml_bytes = archive.read("word/document.xml")

    root = ET.fromstring(xml_bytes)
    lines: list[str] = []
    for paragraph in root.findall(".//w:p", NS):
        runs = [node.text or "" for node in paragraph.findall(".//w:t", NS)]
        text = "".join(runs).strip()
        if text:
            lines.append(text)

    cache_file.parent.mkdir(parents=True, exist_ok=True)
    cache_file.write_text("\n".join(lines), encoding="utf-8")
    return lines


def search_lines(lines: list[str], query_terms: list[str], context: int) -> list[tuple[int, list[str]]]:
    normalized = [term.casefold() for term in query_terms]
    matches: list[tuple[int, list[str]]] = []

    for index, line in enumerate(lines):
        haystack = line.casefold()
        if not all(term in haystack for term in normalized):
            continue

        start = max(0, index - context)
        end = min(len(lines), index + context + 1)
        window = [
            f"{line_index + 1}: {lines[line_index]}"
            for line_index in range(start, end)
        ]
        matches.append((index + 1, window))

    return matches


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Search the implementation spec or whitepapers without manual .docx XML spelunking."
    )
    parser.add_argument(
        "query",
        nargs="*",
        help="Case-insensitive search terms. All terms must match the same extracted line.",
    )
    parser.add_argument(
        "--doc",
        choices=["all", *DOCUMENTS.keys()],
        default="all",
        help="Document to search. Defaults to all tracked spec docs.",
    )
    parser.add_argument(
        "--context",
        type=int,
        default=1,
        help="Number of surrounding extracted lines to print around each match.",
    )
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="Ignore cached extracted text and rebuild it from the .docx file.",
    )
    parser.add_argument(
        "--list-docs",
        action="store_true",
        help="List the known document keys and exit.",
    )
    return parser.parse_args()


def configure_stdio() -> None:
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except ValueError:
            # Some wrapped streams cannot be reconfigured; default encoding will remain.
            pass


def main() -> int:
    configure_stdio()
    args = parse_args()

    if args.list_docs:
        for key, filename in DOCUMENTS.items():
            print(f"{key}: {filename}")
        return 0

    if not args.query:
        print("Provide at least one search term, or use --list-docs.", file=sys.stderr)
        return 2

    doc_keys = list(DOCUMENTS) if args.doc == "all" else [args.doc]
    found_any = False

    for doc_key in doc_keys:
        lines = extract_text(doc_key, refresh=args.refresh)
        matches = search_lines(lines, args.query, context=max(args.context, 0))
        if not matches:
            continue

        found_any = True
        print(f"[{doc_key}] {DOCUMENTS[doc_key]}")
        for _, window in matches:
            print(textwrap.indent("\n".join(window), prefix="  "))
            print()

    if found_any:
        return 0

    print("No matches found.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
