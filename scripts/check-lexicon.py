"""Check the generated merged dictionary; no network or third-party packages."""
import json
import pathlib
import sqlite3
import sys

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".build/lexicon")
db = sqlite3.connect(root / "language-evidence.sqlite")
report = json.loads((root / "lexicon-merge-report.json").read_text())
assert report["name"] == "mimo" and report["version"] == "1.0"
count = db.execute("SELECT count(*) FROM lexicon").fetchone()[0]
assert count == report["final_readings"], (count, report)
assert db.execute("SELECT count(*) FROM (SELECT hans,phonetic,count(*) n FROM lexicon GROUP BY hans,phonetic HAVING n>1)").fetchone()[0] == 0
assert db.execute("SELECT weight FROM lexicon WHERE hant='喵' AND hans='喵' AND reading='miao'").fetchone()[0] == 7000
assert db.execute("SELECT count(DISTINCT phonetic) FROM lexicon WHERE hans='重'").fetchone()[0] >= 2
assert db.execute("SELECT count(*) FROM lexicon WHERE hant='優化' AND hans='优化'").fetchone()[0] > 0
assert db.execute("SELECT count(*) FROM lexicon WHERE hant='喵撲' AND hans='喵扑'").fetchone()[0] == 1
for mode in ("pinyin", "zhuyin"):
    assert "\t喵\t喵\t7000\n" in (root / f"native-{mode}.tsv").read_text()
base_bytes = (root / "mimo_base.bin").read_bytes()
assert base_bytes.startswith(b"MIMO-BASE-1\x00"), "invalid mimo_base.bin magic"
assert len(base_bytes) == report["mimo_base_bin_bytes"]
pair_bytes = (root / "mimo_pairs.bin").read_bytes()
assert pair_bytes.startswith(b"MIMO-PAIR-1\x00") and len(pair_bytes) == report["mimo_pairs_bin_bytes"], "mimo_pairs.bin missing or not from this build"
print(f"PASS: {count:,} merged readings; mimo_base.bin ({len(base_bytes):,} bytes); mimo_pairs.bin ({report['pairs']:,} pairs); no duplicate simplified reading; reviewed weights, polyphony and both native indexes verified")
