#!/usr/bin/env python3
"""Codebase statistics for the Primer repo.

Walks the repo and reports per-language line counts (code / doc-comment /
comment / blank) plus file-length distributions, so you can see at a glance
what's bloated and worth trimming.

Skips git worktrees (auto-detected via `git worktree list`), build artefacts,
caches, virtualenvs, and the vendored crates under `src/vendor/`. Override
with --include-vendor or --no-skip-worktrees if you want them counted.

Usage:
    python scripts/code_stats.py
    python scripts/code_stats.py --top 30 --json out.json
    python scripts/code_stats.py --root src/crates/primer-pedagogy
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

EXT_LANG: dict[str, str] = {
    ".rs": "Rust",
    ".py": "Python",
    ".toml": "TOML",
    ".md": "Markdown",
    ".json": "JSON",
    ".jsonl": "JSONL",
    ".yml": "YAML",
    ".yaml": "YAML",
    ".sh": "Shell",
    ".bash": "Shell",
    ".zsh": "Shell",
    ".txt": "Text",
    ".html": "HTML",
    ".css": "CSS",
    ".js": "JavaScript",
    ".ts": "TypeScript",
    ".sql": "SQL",
}

# Directory names skipped at any depth.
DEFAULT_SKIP_DIRS: set[str] = {
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".worktrees",
    ".claude",
    "dist",
    "build",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".cache",
    ".idea",
    ".vscode",
}

# Path suffixes (relative to repo root) skipped by default.
DEFAULT_SKIP_RELPATHS: set[str] = {
    "src/vendor",
}


@dataclass
class FileStats:
    path: Path
    language: str
    total: int = 0
    blank: int = 0
    comment: int = 0
    doc: int = 0
    code: int = 0


@dataclass
class LangAggregate:
    files: int = 0
    total: int = 0
    blank: int = 0
    comment: int = 0
    doc: int = 0
    code: int = 0
    file_lengths: list[int] = field(default_factory=list)

    def add(self, fs: FileStats) -> None:
        self.files += 1
        self.total += fs.total
        self.blank += fs.blank
        self.comment += fs.comment
        self.doc += fs.doc
        self.code += fs.code
        self.file_lengths.append(fs.total)


# ---------- per-language counters ----------


def count_rust(lines: list[str]) -> tuple[int, int, int, int, int]:
    total = blank = comment = doc = code = 0
    in_block = False
    block_is_doc = False
    for raw in lines:
        total += 1
        s = raw.strip()
        if not s:
            blank += 1
            continue
        if in_block:
            if block_is_doc:
                doc += 1
            else:
                comment += 1
            if "*/" in s:
                in_block = False
                block_is_doc = False
            continue
        if s.startswith("///") or s.startswith("//!"):
            doc += 1
            continue
        if s.startswith("//"):
            comment += 1
            continue
        if s.startswith("/**") and not s.startswith("/**/"):
            doc += 1
            if "*/" not in s[3:]:
                in_block = True
                block_is_doc = True
            continue
        if s.startswith("/*"):
            comment += 1
            if "*/" not in s[2:]:
                in_block = True
                block_is_doc = False
            continue
        code += 1
    return total, blank, comment, doc, code


def count_python(lines: list[str]) -> tuple[int, int, int, int, int]:
    total = blank = comment = doc = code = 0
    open_delim: str | None = None
    for raw in lines:
        total += 1
        s = raw.strip()
        if not s:
            blank += 1
            continue
        if open_delim is not None:
            doc += 1
            if open_delim in s:
                open_delim = None
            continue
        if s.startswith("#"):
            comment += 1
            continue
        for delim in ('"""', "'''"):
            if s.startswith(delim):
                rest = s[len(delim) :]
                if delim in rest:
                    doc += 1
                else:
                    open_delim = delim
                    doc += 1
                break
        else:
            code += 1
    return total, blank, comment, doc, code


def count_hash_comment(lines: list[str]) -> tuple[int, int, int, int, int]:
    """For TOML, YAML, Shell — `#` line comments, no doc-comment concept."""
    total = blank = comment = doc = code = 0
    for raw in lines:
        total += 1
        s = raw.strip()
        if not s:
            blank += 1
        elif s.startswith("#"):
            comment += 1
        else:
            code += 1
    return total, blank, comment, doc, code


def count_markdown(lines: list[str]) -> tuple[int, int, int, int, int]:
    total = blank = comment = doc = code = 0
    in_fence = False
    for raw in lines:
        total += 1
        s = raw.strip()
        if not s:
            blank += 1
            continue
        if s.startswith("```"):
            in_fence = not in_fence
            doc += 1
            continue
        if in_fence:
            code += 1
        else:
            doc += 1
    return total, blank, comment, doc, code


def count_data_only(lines: list[str]) -> tuple[int, int, int, int, int]:
    """JSON / JSONL / plain text: every non-blank line is 'code' (data)."""
    total = blank = comment = doc = code = 0
    for raw in lines:
        total += 1
        if raw.strip():
            code += 1
        else:
            blank += 1
    return total, blank, comment, doc, code


COUNTERS = {
    "Rust": count_rust,
    "Python": count_python,
    "TOML": count_hash_comment,
    "YAML": count_hash_comment,
    "Shell": count_hash_comment,
    "Markdown": count_markdown,
    "JSON": count_data_only,
    "JSONL": count_data_only,
    "Text": count_data_only,
    "SQL": count_hash_comment,
}


# ---------- discovery ----------


def detect_git_worktrees(root: Path) -> list[Path]:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(root), "worktree", "list", "--porcelain"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return []
    paths: list[Path] = []
    for line in out.splitlines():
        if line.startswith("worktree "):
            p = Path(line[len("worktree ") :]).resolve()
            paths.append(p)
    return paths


def iter_files(
    root: Path,
    skip_dirs: set[str],
    skip_relpaths: set[Path],
    skip_worktree_roots: set[Path],
):
    root = root.resolve()

    def walk(d: Path):
        try:
            entries = sorted(d.iterdir(), key=lambda p: p.name)
        except (PermissionError, OSError):
            return
        for e in entries:
            if e.is_symlink():
                continue
            if e.is_dir():
                if e.name in skip_dirs:
                    continue
                if e.resolve() in skip_worktree_roots:
                    continue
                rel = e.resolve().relative_to(root) if e.resolve().is_relative_to(root) else None
                if rel is not None and rel in skip_relpaths:
                    continue
                yield from walk(e)
            elif e.is_file():
                yield e

    yield from walk(root)


def classify(path: Path) -> str | None:
    return EXT_LANG.get(path.suffix.lower())


def stat_file(path: Path, language: str) -> FileStats | None:
    try:
        with path.open("r", encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
    except OSError:
        return None
    counter = COUNTERS.get(language, count_data_only)
    total, blank, comment, doc, code = counter(lines)
    return FileStats(
        path=path,
        language=language,
        total=total,
        blank=blank,
        comment=comment,
        doc=doc,
        code=code,
    )


# ---------- reporting ----------


def fmt_int(n: int) -> str:
    return f"{n:,}"


def percentile(values: list[int], p: float) -> int:
    if not values:
        return 0
    s = sorted(values)
    k = (len(s) - 1) * p
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    frac = k - lo
    return int(round(s[lo] + (s[hi] - s[lo]) * frac))


def print_report(
    files: list[FileStats],
    by_lang: dict[str, LangAggregate],
    top_n: int,
) -> None:
    grand = LangAggregate()
    for fs in files:
        grand.add(fs)

    print("=" * 78)
    print("Primer codebase statistics")
    print("=" * 78)
    print(f"Files counted        : {fmt_int(grand.files)}")
    print(f"Total lines          : {fmt_int(grand.total)}")
    print(f"  Code               : {fmt_int(grand.code)}  ({pct(grand.code, grand.total)})")
    print(f"  Doc / docstrings   : {fmt_int(grand.doc)}  ({pct(grand.doc, grand.total)})")
    print(f"  Comments           : {fmt_int(grand.comment)}  ({pct(grand.comment, grand.total)})")
    print(f"  Blank              : {fmt_int(grand.blank)}  ({pct(grand.blank, grand.total)})")
    print()

    print("-" * 78)
    print("Per-language breakdown (sorted by total lines)")
    print("-" * 78)
    header = f"{'Language':<12} {'Files':>6} {'Total':>9} {'Code':>9} {'Doc':>8} {'Comm':>7} {'Blank':>8}"
    print(header)
    print("-" * len(header))
    for lang, agg in sorted(by_lang.items(), key=lambda kv: -kv[1].total):
        print(
            f"{lang:<12} "
            f"{fmt_int(agg.files):>6} "
            f"{fmt_int(agg.total):>9} "
            f"{fmt_int(agg.code):>9} "
            f"{fmt_int(agg.doc):>8} "
            f"{fmt_int(agg.comment):>7} "
            f"{fmt_int(agg.blank):>8}"
        )
    print()

    print("-" * 78)
    print("File-length distribution per language (lines per file)")
    print("-" * 78)
    header = f"{'Language':<12} {'Files':>6} {'Mean':>7} {'Median':>7} {'P90':>7} {'P99':>7} {'Max':>7}"
    print(header)
    print("-" * len(header))
    for lang, agg in sorted(by_lang.items(), key=lambda kv: -kv[1].total):
        if not agg.file_lengths:
            continue
        lengths = agg.file_lengths
        print(
            f"{lang:<12} "
            f"{fmt_int(agg.files):>6} "
            f"{int(statistics.mean(lengths)):>7} "
            f"{int(statistics.median(lengths)):>7} "
            f"{percentile(lengths, 0.90):>7} "
            f"{percentile(lengths, 0.99):>7} "
            f"{max(lengths):>7}"
        )
    print()

    print("-" * 78)
    print(f"Longest files (top {top_n})")
    print("-" * 78)
    longest = sorted(files, key=lambda fs: -fs.total)[:top_n]
    width = max((len(str(fs.path)) for fs in longest), default=0)
    for fs in longest:
        print(
            f"{fs.total:>6}  {fs.language:<10} "
            f"code={fs.code:<5} doc={fs.doc:<4} comm={fs.comment:<4} blank={fs.blank:<4}  "
            f"{fs.path}"
        )


def pct(part: int, whole: int) -> str:
    if whole == 0:
        return "0.0%"
    return f"{100 * part / whole:.1f}%"


def to_json(
    files: list[FileStats],
    by_lang: dict[str, LangAggregate],
    top_n: int,
) -> dict:
    grand = LangAggregate()
    for fs in files:
        grand.add(fs)
    return {
        "totals": {
            "files": grand.files,
            "total": grand.total,
            "code": grand.code,
            "doc": grand.doc,
            "comment": grand.comment,
            "blank": grand.blank,
        },
        "languages": {
            lang: {
                "files": a.files,
                "total": a.total,
                "code": a.code,
                "doc": a.doc,
                "comment": a.comment,
                "blank": a.blank,
                "length_mean": (
                    int(statistics.mean(a.file_lengths)) if a.file_lengths else 0
                ),
                "length_median": (
                    int(statistics.median(a.file_lengths)) if a.file_lengths else 0
                ),
                "length_p90": percentile(a.file_lengths, 0.90),
                "length_p99": percentile(a.file_lengths, 0.99),
                "length_max": max(a.file_lengths) if a.file_lengths else 0,
            }
            for lang, a in by_lang.items()
        },
        "longest": [
            {
                "path": str(fs.path),
                "language": fs.language,
                "total": fs.total,
                "code": fs.code,
                "doc": fs.doc,
                "comment": fs.comment,
                "blank": fs.blank,
            }
            for fs in sorted(files, key=lambda fs: -fs.total)[:top_n]
        ],
    }


# ---------- main ----------


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent

    ap = argparse.ArgumentParser(description="Codebase statistics for the Primer repo.")
    ap.add_argument(
        "--root",
        type=Path,
        default=repo_root,
        help="Directory to scan (default: repo root).",
    )
    ap.add_argument(
        "--top",
        type=int,
        default=20,
        help="Show the top N longest files (default: 20).",
    )
    ap.add_argument(
        "--include-vendor",
        action="store_true",
        help="Include src/vendor (vendored crates) in the stats.",
    )
    ap.add_argument(
        "--no-skip-worktrees",
        action="store_true",
        help="Do not skip git worktrees discovered via `git worktree list`.",
    )
    ap.add_argument(
        "--json",
        type=Path,
        help="Also write a machine-readable JSON report to this path.",
    )
    args = ap.parse_args()

    root = args.root.resolve()
    if not root.is_dir():
        print(f"error: {root} is not a directory", file=sys.stderr)
        return 2

    skip_dirs = set(DEFAULT_SKIP_DIRS)
    skip_relpaths = {Path(p) for p in DEFAULT_SKIP_RELPATHS}
    if args.include_vendor:
        skip_relpaths.discard(Path("src/vendor"))

    skip_worktree_roots: set[Path] = set()
    if not args.no_skip_worktrees:
        for wt in detect_git_worktrees(root):
            if wt != root:
                skip_worktree_roots.add(wt)

    files: list[FileStats] = []
    by_lang: dict[str, LangAggregate] = {}

    for path in iter_files(root, skip_dirs, skip_relpaths, skip_worktree_roots):
        lang = classify(path)
        if lang is None:
            continue
        fs = stat_file(path, lang)
        if fs is None:
            continue
        try:
            fs.path = fs.path.resolve().relative_to(root)
        except ValueError:
            pass
        files.append(fs)
        by_lang.setdefault(lang, LangAggregate()).add(fs)

    print_report(files, by_lang, args.top)

    if args.json:
        report = to_json(files, by_lang, args.top)
        args.json.write_text(json.dumps(report, indent=2, default=str), encoding="utf-8")
        print(f"\nWrote JSON report to {args.json}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
