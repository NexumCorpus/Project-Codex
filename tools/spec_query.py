#!/usr/bin/env python3
"""Search the Nexum Graph specification and whitepaper .docx files."""

from __future__ import annotations

import argparse
import json
import re
import sys
import textwrap
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass
from pathlib import Path

NS = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}

DOCUMENTS = {
    "spec": "Project_Codex_Final_Implementation_Spec.docx",
    "whitepaper-v1": "Project_Codex_Whitepaper_v1.docx",
    "whitepaper-v3": "Project_Codex_Whitepaper_v3.docx",
}


@dataclass(frozen=True)
class DocumentState:
    """Cached document metadata."""

    doc_key: str
    doc_path: Path
    cache_path: Path
    lines: list[str]
    from_cache: bool


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def cache_dir() -> Path:
    return repo_root() / ".nex" / "cache" / "spec-text"


def cache_text_path(doc_key: str) -> Path:
    return cache_dir() / f"{doc_key}.txt"


def cache_meta_path(doc_key: str) -> Path:
    return cache_dir() / f"{doc_key}.json"


def document_path(doc_key: str) -> Path:
    return repo_root() / DOCUMENTS[doc_key]


def read_cached_lines(doc_key: str, source_path: Path) -> list[str] | None:
    text_path = cache_text_path(doc_key)
    meta_path = cache_meta_path(doc_key)
    if not text_path.exists() or not meta_path.exists():
        return None

    try:
        metadata = json.loads(meta_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return None

    current_mtime = source_path.stat().st_mtime_ns
    if metadata.get("source_mtime_ns") != current_mtime:
        return None

    return text_path.read_text(encoding="utf-8").splitlines()


def write_cache(doc_key: str, source_path: Path, lines: list[str]) -> None:
    cache_dir().mkdir(parents=True, exist_ok=True)
    cache_text_path(doc_key).write_text("\n".join(lines), encoding="utf-8")
    cache_meta_path(doc_key).write_text(
        json.dumps(
            {
                "doc_key": doc_key,
                "source_path": str(source_path),
                "source_mtime_ns": source_path.stat().st_mtime_ns,
                "line_count": len(lines),
            },
            indent=2,
        ),
        encoding="utf-8",
    )


def extract_text(doc_key: str, refresh: bool) -> DocumentState:
    source_path = document_path(doc_key)
    if not source_path.exists():
        raise FileNotFoundError(f"missing document: {source_path}")

    if not refresh:
        cached_lines = read_cached_lines(doc_key, source_path)
        if cached_lines is not None:
            return DocumentState(
                doc_key=doc_key,
                doc_path=source_path,
                cache_path=cache_text_path(doc_key),
                lines=cached_lines,
                from_cache=True,
            )

    with zipfile.ZipFile(source_path) as archive:
        xml_bytes = archive.read("word/document.xml")

    root = ET.fromstring(xml_bytes)
    lines: list[str] = []
    for paragraph in root.findall(".//w:p", NS):
        runs = [node.text or "" for node in paragraph.findall(".//w:t", NS)]
        text = "".join(runs).strip()
        if text:
            lines.append(text)

    write_cache(doc_key, source_path, lines)
    return DocumentState(
        doc_key=doc_key,
        doc_path=source_path,
        cache_path=cache_text_path(doc_key),
        lines=lines,
        from_cache=False,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Search the implementation spec or whitepapers without manual .docx XML spelunking."
    )
    parser.add_argument(
        "query",
        nargs="*",
        help="Search terms or regex pattern, depending on --mode.",
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
    parser.add_argument(
        "--mode",
        choices=["all", "any", "phrase", "regex"],
        default="all",
        help="Search mode. Defaults to all-term match on one extracted line.",
    )
    parser.add_argument(
        "--max-results",
        type=int,
        default=20,
        help="Maximum matches to print per document. Defaults to 20.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON instead of text.",
    )
    parser.add_argument(
        "--stats",
        action="store_true",
        help="Print cache/source metadata with results.",
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
            pass


def matches_line(line: str, query_terms: list[str], mode: str) -> bool:
    haystack = line.casefold()
    if mode == "all":
        return all(term.casefold() in haystack for term in query_terms)
    if mode == "any":
        return any(term.casefold() in haystack for term in query_terms)
    if mode == "phrase":
        phrase = " ".join(query_terms).casefold()
        return phrase in haystack
    pattern = re.compile(query_terms[0], re.IGNORECASE)
    return pattern.search(line) is not None


def search_lines(lines: list[str], query_terms: list[str], context: int, mode: str) -> list[dict]:
    matches: list[dict] = []
    for index, line in enumerate(lines):
        if not matches_line(line, query_terms, mode):
            continue

        start = max(0, index - context)
        end = min(len(lines), index + context + 1)
        window = [f"{line_index + 1}: {lines[line_index]}" for line_index in range(start, end)]
        matches.append(
            {
                "line_number": index + 1,
                "line": line,
                "window": window,
            }
        )
    return matches


def print_text_result(document: DocumentState, matches: list[dict], include_stats: bool) -> None:
    source_label = "cache" if document.from_cache else "docx"
    suffix = ""
    if include_stats:
        suffix = (
            f" ({len(document.lines)} lines, source={source_label}, "
            f"cache={document.cache_path.relative_to(repo_root())})"
        )
    print(f"[{document.doc_key}] {DOCUMENTS[document.doc_key]}{suffix}")
    for match in matches:
        print(textwrap.indent("\n".join(match["window"]), prefix="  "))
        print()


def print_json_result(results: list[dict]) -> None:
    print(json.dumps(results, indent=2))


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

    context = max(args.context, 0)
    doc_keys = list(DOCUMENTS) if args.doc == "all" else [args.doc]
    results: list[dict] = []

    for doc_key in doc_keys:
        document = extract_text(doc_key, refresh=args.refresh)
        matches = search_lines(document.lines, args.query, context=context, mode=args.mode)
        if args.max_results >= 0:
            matches = matches[: args.max_results]
        if not matches:
            continue

        results.append(
            {
                "doc_key": doc_key,
                "filename": DOCUMENTS[doc_key],
                "doc_path": str(document.doc_path),
                "cache_path": str(document.cache_path),
                "line_count": len(document.lines),
                "from_cache": document.from_cache,
                "matches": matches,
            }
        )

    if not results:
        if args.json:
            print_json_result([])
        else:
            print("No matches found.")
        return 1

    if args.json:
        print_json_result(results)
    else:
        for result in results:
            document = DocumentState(
                doc_key=result["doc_key"],
                doc_path=Path(result["doc_path"]),
                cache_path=Path(result["cache_path"]),
                lines=[""] * result["line_count"],
                from_cache=result["from_cache"],
            )
            print_text_result(document, result["matches"], include_stats=args.stats)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
