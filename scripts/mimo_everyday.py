"""Sentences written for this project, cleaned into DataSources/mimo-everyday.

  python3 scripts/mimo_everyday.py RAW_DIR
      The first batch (2026-11-11), everyday chat. RAW_DIR holds the writers' files (*.txt, one
      sentence per line; tNN = Taiwan topic, cNN = mainland topic). Writes everyday.txt.xz.
  python3 scripts/mimo_everyday.py more RAW_DIR
      The second batch (2026-11-12). RAW_DIR/chat/*.txt: chat lines, things said in everyday
      situations, and question and answer pairs. RAW_DIR/written/*.txt: narration, description,
      explanation, stories and example sentences. RAW_DIR/mixed/*.txt: Chinese with English words
      in it, as people write at work and in chat. File names start with t (Taiwan, Traditional)
      or c (mainland, Simplified). Writes chat-2.txt.xz, written-2.txt.xz and mixed-2.txt.xz; a
      line the first batch already has is dropped.

Lines are stripped and dropped when empty, without a Han character, longer than 40 characters,
containing a control or replacement character, or an exact duplicate of an earlier line. The
kept lines stay in file-name order; SOURCE.json records the counts and the checksums.
scripts/mimo_pairs.py counts everyday.txt.xz and chat-2.txt.xz as spoken text and
written-2.txt.xz and mixed-2.txt.xz as written text.
"""
import hashlib
import json
import lzma
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "DataSources/mimo-everyday"
HAN = re.compile(r"[㐀-䶿一-鿿]")
BROKEN = re.compile(r"[\x00-\x1f\x7f�]")
MAX_CHARS = 40


def clean(raw_dir: Path, seen=None):
    kept, dropped, per_region = [], Counter(), Counter()
    seen = set() if seen is None else seen
    files = sorted(raw_dir.glob("*.txt"))
    for path in files:
        for line in path.read_text(encoding="utf-8").splitlines():
            text = line.strip()
            if not text:
                dropped["empty"] += 1
            elif BROKEN.search(text) or not HAN.search(text):
                dropped["no Han character or broken"] += 1
            elif len(text) > MAX_CHARS:
                dropped[f"over {MAX_CHARS} characters"] += 1
            elif text in seen:
                dropped["exact duplicate"] += 1
            else:
                seen.add(text)
                kept.append(text)
                per_region["taiwan" if path.name.startswith("t") else "mainland"] += 1
    return files, kept, dropped, per_region


def main(raw_dir: str):
    files, kept, dropped, per_region = clean(Path(raw_dir))
    OUT.mkdir(parents=True, exist_ok=True)
    data = OUT / "everyday.txt.xz"
    with lzma.open(data, "wt", encoding="utf-8", preset=9) as f:
        f.write("\n".join(kept) + "\n")
    manifest = {
        "name": "mimo-everyday",
        "description": "Short everyday chat sentences in Chinese, Taiwan usage in Traditional characters and "
                       "mainland usage in Simplified characters. Written on 2026-11-11 by language-model runs "
                       "for this project, one writer per topic; the writers were not shown the benchmark or "
                       "the held-out sets. Used as spoken-style text for the word-pair counts and the word "
                       "weights (scripts/mimo_pairs.py).",
        "license": "written for this project; no third-party text",
        "author": "Meowpuut project (machine-written, cleaned by scripts/mimo_everyday.py)",
        "topics": {
            "taiwan": "meals and ordering; convenience stores, markets, paying; commuting and transport; school; "
                      "work; online shopping; health; weather; family and home; meeting friends; relationships; "
                      "moods and complaints; phones, computers, internet; anime and manga; games; film, music, "
                      "concerts, streamers; travel; money; pets; exercise and diet; renting and moving; short "
                      "replies and reactions; requests, thanks, apologies; questions; time, dates, numbers, "
                      "arrangements; holidays and life events; cooking; appearance and clothes; learning "
                      "Japanese and Japan; small talk",
            "mainland": "meals and ordering; commuting and transport; school; work; online shopping and parcels; "
                        "health; family and home; meeting friends; relationships; moods; phones, computers, "
                        "internet; anime, manga and games; film, music, variety shows, streamers; travel; money; "
                        "short replies and reactions; requests, thanks, apologies and questions; time, dates, "
                        "numbers, arrangements and life events",
        },
        "counts": {"writer_files": len(files), "lines_read": len(kept) + sum(dropped.values()),
                   "lines_kept": len(kept), "kept_taiwan": per_region["taiwan"],
                   "kept_mainland": per_region["mainland"], "dropped": dict(dropped)},
        "files": {"everyday.txt.xz": hashlib.sha256(data.read_bytes()).hexdigest()},
    }
    (OUT / "SOURCE.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(manifest["counts"], ensure_ascii=False))


def more(raw_dir: str):
    with lzma.open(OUT / "everyday.txt.xz", "rt", encoding="utf-8") as f:
        seen = set(f.read().splitlines())
    manifest = json.loads((OUT / "SOURCE.json").read_text())
    batch = {"written": "2026-11-12, by language-model runs for this project; the writers were given a topic, a "
                        "register and content limits (no politics, nothing aimed at a group of people, no "
                        "real persons, works or brands) and were not shown the benchmark or the held-out sets"}
    for kind in ("chat", "written", "mixed"):
        files, kept, dropped, per_region = clean(Path(raw_dir) / kind, seen)
        data = OUT / f"{kind}-2.txt.xz"
        with lzma.open(data, "wt", encoding="utf-8", preset=9) as f:
            f.write("\n".join(kept) + "\n")
        batch[kind] = {"writer_files": len(files), "lines_kept": len(kept), "kept_taiwan": per_region["taiwan"],
                       "kept_mainland": per_region["mainland"], "dropped": dict(dropped)}
        manifest["files"][data.name] = hashlib.sha256(data.read_bytes()).hexdigest()
    manifest["batch_2"] = batch
    (OUT / "SOURCE.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(batch, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "more":
        more(sys.argv[2])
        sys.exit()
    main(*sys.argv[1:])
