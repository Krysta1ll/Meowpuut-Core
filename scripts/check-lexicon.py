"""Check the generated merged dictionary; no network or third-party packages."""
import collections
import importlib.util
import json
import re
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
# The never-volunteer list (Resources/Lexicon/blocked-terms.txt): no emoji association for a listed
# word and no flag, no context row with a listed term, no initials row of a marked word.
scripts = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(scripts))
import blocked_terms  # noqa: E402
spec = importlib.util.spec_from_file_location("build_emoji_associations", scripts / "build-emoji-associations.py")
emoji = importlib.util.module_from_spec(spec)
spec.loader.exec_module(emoji)
associations = json.loads((root / "associations.json").read_text(encoding="utf-8"))
bad = [(lang, word) for lang, entries in associations.items() for word, entry in entries.items()
       if emoji.never_offered(lang, word) or any(emoji.banned(symbol) for symbol in entry[0])]
assert not bad, f"associations.json: {len(bad)} keys with a listed word or a banned emoji"
marked, abbreviated, context, keep = set(), [], [], set()
with open(root / "native-pinyin.tsv", encoding="utf-8") as rows:
    next(rows)
    in_context = False
    for line in rows:
        fields = line.rstrip("\n").split("\t")
        if fields[0] == "# context":
            in_context = True
        elif in_context:
            context.append(fields[0])
        elif len(fields) == 6 and fields[5] == "k":
            keep.add(fields[3])
        elif len(fields) == 6:
            marked.add(fields[3])
            if fields[0].islower() and not re.fullmatch(r"([a-z]*[aeiouv][a-z]*[1-5])+", fields[1]):
                abbreviated.append(fields[0])
assert not abbreviated, f"{len(abbreviated)} initials rows of never-volunteer words"
simplify = emoji.simplified_characters()
listed_context = [p for p in context if blocked_terms.zh_hit("".join(simplify.get(c, c) for c in p))]
assert not listed_context, f"{len(listed_context)} context rows with a listed term"
assert all(blocked_terms.zh_hit(text) for text in marked if not text.isascii()), "a marked row without a listed term"
# Every [zh] term is a marked word of its own (the engine knows the list by its marked rows).
unmarked = [t for t in blocked_terms.zh_terms() if len(t) >= 2 and t not in marked]
assert not unmarked, f"{len(unmarked)} [zh] terms without a marked row of their own"
assert not any(blocked_terms.zh_hit(text) for text in keep), "a [zh-keep] row that is still hit"
# Homophone order: a row of the authored overlays (Resources/Lexicon/chinese.tsv, community.tsv) may not
# outrank a word of the same bare reading (tones ignored) whose index weight is 5000 or more, unless the
# line directly after the row is the marker `#outranks-ok` (a reviewed judgement that the row is the more
# everyday choice). A row with the same Traditional form is the same word, not a homophone. Weights are
# those of the built pinyin index (.build/lexicon/native-pinyin.tsv).
OUTRANK_MARKER = "#outranks-ok"
index_rows = collections.defaultdict(list)
with open(root / "native-pinyin.tsv", encoding="utf-8") as rows:
    next(rows)
    for line in rows:
        if line.startswith("# context"): break
        fields = line.rstrip("\n").split("\t")
        # A sixth field is a row mark (w, n, k of compile-native-lexicon.py); the row still counts.
        if len(fields) in (5, 6): index_rows[fields[0]].append((int(fields[4]), fields[1], fields[2], fields[3]))
overlay_dir = scripts.parent / "Resources/Lexicon"
outranking, marked_rows = [], 0
for name in ("chinese.tsv", "community.tsv"):
    lines = (overlay_dir / name).read_text(encoding="utf-8").splitlines()
    for number, line in enumerate(lines, 1):
        if not line or line.startswith("#"): continue
        fields = line.split("|")
        if len(fields) != 4: continue
        hant, hans, code, priority = fields
        bare = "".join(re.sub(r"[1-5]", "", sound) for sound in code.split())
        weight = max([w for w, _, t, s in index_rows.get(bare, []) if t == hant and s == hans], default=int(priority))
        rivals = [(w, s) for w, _, t, s in index_rows.get(bare, []) if s != hans and t != hant and 5000 <= w < weight]
        if not rivals: continue
        if lines[number].strip() == OUTRANK_MARKER:
            marked_rows += 1
        else:
            top = max(rivals)
            outranking.append(f"{name}:{number} {hant}|{hans}|{code} at {weight} over {top[1]} {top[0]}")
assert not outranking, f"{len(outranking)} overlay rows outrank a heavier homophone without {OUTRANK_MARKER}: " + "; ".join(outranking[:20])
print(f"PASS: never-volunteer list: {len(marked):,} marked words, no initials rows, no context rows, emoji clean")
print(f"PASS: {count:,} merged readings; mimo_base.bin ({len(base_bytes):,} bytes); mimo_pairs.bin ({report['pairs']:,} pairs); no duplicate simplified reading; reviewed weights, polyphony and both native indexes verified")
