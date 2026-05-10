"""Tests for read_whitelist — reads a text file, returns ordered titles."""
import pytest
from pathlib import Path
from wiki.source import read_whitelist


def test_basic_titles(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Photosynthesis\nBlack hole\nGravity\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole", "Gravity"]


def test_comments_skipped(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("# header\nPhotosynthesis\n# inline comment\nBlack hole\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_blank_lines_skipped(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("\nPhotosynthesis\n\n\nBlack hole\n\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_whitespace_trimmed(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("  Photosynthesis  \n\tBlack hole\t\n")
    assert read_whitelist(p) == ["Photosynthesis", "Black hole"]


def test_order_preserved(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Zebra\nAlpha\nMango\n")
    assert read_whitelist(p) == ["Zebra", "Alpha", "Mango"]


def test_duplicates_raise(tmp_path: Path):
    p = tmp_path / "wl.txt"
    p.write_text("Photosynthesis\nBlack hole\nPhotosynthesis\n")
    with pytest.raises(ValueError, match="duplicate"):
        read_whitelist(p)
