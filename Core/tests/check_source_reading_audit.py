#!/usr/bin/env python3
"""Audit source reading contradictions without discarding standalone entries."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / "scripts"))
spec = importlib.util.spec_from_file_location("mimo_compiler", root / "scripts/compile-native-lexicon.py")
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

assert compiler.inspect_source_reading("ㄅㄧㄣ", "bin1", "1up")[2] == []
assert compiler.inspect_source_reading("ㄅㄧㄣ", "bin2", "1up")[2] == ["tone_conflict"]
assert compiler.inspect_source_reading("ㄧㄜˋ", "ye4", "uk4")[2] == ["sound_conflict"]
assert compiler.inspect_source_reading("ㄍㄨㄥ˙", "gong5", "ej/5")[2] == ["key_conflict"]
assert compiler.inspect_source_reading("ㄅ?", "bin1", "1up")[2] == ["invalid_bopomofo"]
for alias, canonical in (("lue4", "lve4"), ("nue4", "nve4")):
    assert compiler.canonical_pinyin(alias) == canonical
    assert compiler.encode(alias) == compiler.encode(canonical)
assert compiler.numbered_pinyin("lüè") == "lve4"
assert compiler.numbered_pinyin("nüè") == "nve4"

with tempfile.TemporaryDirectory(prefix="mimo-source-audit-") as temporary:
    fixture_root = Path(temporary)
    source = fixture_root / "DataSources/McBopomofo"
    source.mkdir(parents=True)
    reviewed_dir = fixture_root / "Resources/Lexicon"
    reviewed_dir.mkdir(parents=True)
    reviewed = {"version": 1, "rules": [{
        "id": "reviewed-yi", "source": "McBopomofo/BPMFBase.txt",
        "source_line": "意 ㄧㄜˋ ye4 uk4 big5", "word": "意", "pinyin": "ye4",
        "reason": "reviewed", "evidence": "https://dict.revised.moe.edu.tw/dictView.jsp?ID=10675",
    }]}
    reviewed_path = reviewed_dir / "source-reading-exclusions.json"
    reviewed_path.write_text(json.dumps(reviewed, ensure_ascii=False), encoding="utf-8")
    (source / "SOURCE.json").write_text(json.dumps({"files": {}, "reading_exclusions": {
        "file": "Resources/Lexicon/source-reading-exclusions.json",
        "sha256": hashlib.sha256(reviewed_path.read_bytes()).hexdigest(),
    }}), encoding="utf-8")
    (source / "BPMFBase.txt").write_text(
        "玢 ㄅㄧㄣ bin2 1up big5\n"
        "賓 ㄅㄧㄣ bin 1up big5\n"
        "意 ㄧㄜˋ ye4 uk4 big5\n"
        "意 ㄧˋ yi4 u4 big5\n"
        "公 ㄍㄨㄥ˙ gong5 ej/5 big5\n"
        "略 ㄌㄩㄝˋ lue4 xm,4 big5\n"
        "虐 ㄋㄩㄝˋ nue4 sm,4 big5\n", encoding="utf-8")
    (source / "BPMFMappings.txt").write_text(
        "賓玢 ㄅㄧㄣ ㄅㄧㄣ\n"
        "公公 ㄍㄨㄥ˙ ㄍㄨㄥ˙\n"
        "意意 ㄧㄜˋ ㄧㄜˋ\n"
        "略虐 ㄌㄩㄝˋ ㄋㄩㄝˋ\n", encoding="utf-8")
    (source / "phrase.occ").write_text("", encoding="utf-8")
    (source / "heterophony1.list").write_text("", encoding="utf-8")
    audit = {}
    with patch.object(compiler, "ROOT", fixture_root), \
         patch.object(compiler, "convert_script", side_effect=lambda converter, words, traditional=False: list(words)), \
         contextlib.redirect_stdout(io.StringIO()):
        words = compiler.imported_words("fixture", audit)
    # A conflicting standalone row remains, but cannot poison the phrase map.
    assert ("玢", "玢", "bin2") in words
    assert ("賓玢", "賓玢", "bin1 bin1") in words
    # A source-key typo alone doesn't invalidate independently matching readings.
    assert ("公公", "公公", "gong5 gong5") in words
    assert ("略", "略", "lve4") in words
    assert ("略虐", "略虐", "lve4 nve4") in words
    assert ("意", "意", "yi4") in words
    assert ("意", "意", "ye4") not in words
    assert not any(key[0] == "意意" for key in words)
    assert audit["issue_counts"] == {"key_conflict": 1, "sound_conflict": 1, "tone_conflict": 1}
    assert audit["unmapped_phrase_rows"] == 1
    assert audit["unmapped_phrases"][0]["word"] == "意意"
    assert len(audit["issues"]) == 3
    assert [item["id"] for item in audit["reviewed_exclusions"]["matched"]] == ["reviewed-yi"]
    assert audit["reviewed_exclusions"]["missing_rule_ids"] == []

    # A later upstream snapshot may remove the exact row: report the stale
    # rule, and never suppress a different source row for the same character.
    base = source / "BPMFBase.txt"
    base.write_text(base.read_text(encoding="utf-8").replace("意 ㄧㄜˋ ye4 uk4 big5\n", ""), encoding="utf-8")
    missing = {}
    with patch.object(compiler, "ROOT", fixture_root), \
         patch.object(compiler, "convert_script", side_effect=lambda converter, words, traditional=False: list(words)), \
         contextlib.redirect_stdout(io.StringIO()):
        remaining = compiler.imported_words("fixture", missing)
    assert ("意", "意", "yi4") in remaining
    assert missing["reviewed_exclusions"]["matched"] == []
    assert missing["reviewed_exclusions"]["missing_rule_ids"] == ["reviewed-yi"]

    # An independently provided reading from the other source is not removed
    # by a rule scoped to one exact McBopomofo line.
    pinyin_source = fixture_root / "DataSources/PinyinPro"
    pinyin_source.mkdir(parents=True)
    (pinyin_source / "SOURCE.json").write_text(json.dumps({"files": {}}), encoding="utf-8")
    (pinyin_source / "complete.json").write_text(json.dumps({"意": ["yè", 1e-9]}, ensure_ascii=False), encoding="utf-8")
    with patch.object(compiler, "ROOT", fixture_root), \
         patch.object(compiler, "convert_script", side_effect=lambda converter, words, traditional=False: list(words)), \
         contextlib.redirect_stdout(io.StringIO()):
        merged, _, _ = compiler.merge_simplified(remaining, "fixture")
    assert ("意", "意", "ye4") in merged

print("PASS: source reading audit, conflicting mapping quarantine and standalone preservation")
