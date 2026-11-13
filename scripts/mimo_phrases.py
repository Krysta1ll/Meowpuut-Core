"""Phrases: short runs of lexicon words that the sources write often (知道了, 是一个, 毕业以后).

The word lists give words. People also type a few words in one go, and a run
that is common enough is worth a lexicon row of its own: it is offered as one
candidate, it has an initials key (zdl), and the decoder pays for one word
instead of two or three. compile-native-lexicon.py adds the phrases of
DataSources/mimo-phrases/phrases.tsv to the lexicon, each read as its words
are read.

  python3 scripts/mimo_phrases.py count LEXICON_DIR [--exclude FILE.jsonl ...] [--jobs 4]
      LEXICON_DIR is a lexicon compiled without phrases
      (compile-native-lexicon.py OUT CONVERTER --no-phrases). Reads the sources
      of DataSources/mimo-pairs (scripts/mimo_pairs.py fetch), splits every
      clause into the words of that lexicon and counts each run of 2 or more
      words that is 2 to 4 characters long. The --exclude files are the test
      and held-out sets, as for the pair counts. Counts go to
      .build/pair-corpus/phrases. About 10 minutes with 4 jobs.
  python3 scripts/mimo_phrases.py snapshot
      Adds the sources up, keeps the phrases the rule of snapshot() names
      and writes phrases.tsv and SOURCE.json.
"""
import argparse
import json
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

from mimo_pairs import CACHE, SPOKEN_WEIGHT, UNKNOWN, blocked_terms, clauses, lexicon_from_dir, sha256, sources, word_counts

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-phrases"
PHRASES_FILE = DATA / "phrases.tsv"
COUNTS = CACHE / "phrases"
MAX_CHARACTERS = 4
TABLE_LIMIT = 12_000_000  # runs held while counting a source; runs seen once are dropped above this
WRITTEN_MIN = 2           # a written source keeps runs seen this often
MIN_WHOLE = 5      # whole clauses of spoken text a phrase needs
WHOLE_SHARE = 0.2  # and that share of its spoken uses


def count_source(name: str, lexicon_dir: str, exclude: list[str]):
    lex = lexicon_from_dir(lexicon_dir)
    forms = lex.forms
    counts, whole, stats = Counter(), Counter(), Counter()
    found, _, _ = clauses(name, lex, exclude, stats)
    for segment, runs in found:
        for run in runs:
            words = [forms[i] if i != UNKNOWN else None for i in lex.split(run)]
            if 1 < len(words) and len(run) <= MAX_CHARACTERS and run == segment and None not in words:
                whole[" ".join(words)] += 1
            for i, first in enumerate(words):
                if first is None:
                    continue
                size, j = len(first), i + 1
                while j < len(words) and words[j] is not None and size + len(words[j]) <= MAX_CHARACTERS:
                    size += len(words[j])
                    j += 1
                    counts[" ".join(words[i:j])] += 1
        if len(counts) > TABLE_LIMIT:
            counts = Counter({run: n for run, n in counts.items() if n > 1})
            stats["times_single_runs_dropped"] += 1
    least = 1 if sources()[name]["group"] == "spoken" else WRITTEN_MIN
    COUNTS.mkdir(parents=True, exist_ok=True)
    with open(COUNTS / f"{name}.tsv", "w", encoding="utf-8") as f:
        for run, n in sorted(counts.items()):
            if n >= least:
                f.write(f"{run}\t{n}\t{whole[run]}\n")
                stats["runs"] += 1
    (COUNTS / f"{name}.json").write_text(json.dumps({"source": name, "fingerprint": lex.fingerprint, "stats": dict(stats)}, indent=1))
    return name, dict(stats)


def merged_counts():
    """text -> [written count, spoken count, spoken count as a whole clause, {split: count}] over all sources."""
    merged = {}
    for name, src in sources().items():
        spoken = src["group"] == "spoken"
        with open(COUNTS / f"{name}.tsv", encoding="utf-8") as f:
            for line in f:
                run, n, whole = line.rstrip("\n").split("\t")
                n = int(n)
                item = merged.setdefault(run.replace(" ", ""), [0, 0, 0, {}])
                item[spoken] += n
                item[2] += int(whole) * spoken
                item[3][run] = item[3].get(run, 0) + (SPOKEN_WEIGHT if spoken else 1) * n
    return merged


def snapshot(out: Path = PHRASES_FILE):
    """Keeps a run that people say on its own: it is a whole clause of the spoken sources
    MIN_WHOLE times or more, and at least WHOLE_SHARE of its spoken uses are whole clauses
    (辛苦 了, 知道 了, 真的 吗, 别 担心). A run that is common only inside longer sentences
    (我 的, 的 时候) stays out: the word pairs already cover it, and as a word of its own it
    would crowd out real words with the same sound."""
    counted = word_counts()
    blocked = blocked_terms()
    kept = []
    for text, (written, spoken, whole, splits) in merged_counts().items():
        words = max(splits, key=splits.get).split(" ")
        if text in counted or any(word not in counted for word in words) or blocked.search(text):
            continue
        if whole >= MIN_WHOLE and whole >= WHOLE_SHARE * spoken:
            kept.append((" ".join(words), written, spoken))
    kept.sort(key=lambda row: (-row[1] - SPOKEN_WEIGHT * row[2], row[0]))
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        f.write("# words of the phrase, count in written text, count in spoken text (scripts/mimo_phrases.py)\n")
        for words, written, spoken in kept:
            f.write(f"{words}\t{written}\t{spoken}\n")
    if out == PHRASES_FILE:
        manifest = {
            "name": "mimo-phrases",
            "description": "Short runs of lexicon words that people say on their own (辛苦了, 知道了, 真的吗), found by scripts/mimo_phrases.py in the spoken texts of DataSources/mimo-pairs and added to the lexicon at build time.",
            "license": "The list is the project's own work; sources and their terms as in DataSources/mimo-pairs/SOURCE.json.",
            "rule": f"a whole clause of the spoken sources {MIN_WHOLE} times or more, and at least {WHOLE_SHARE} of its spoken uses are whole clauses; 2 to {MAX_CHARACTERS} characters; no blocked term",
            "phrases": len(kept),
            "files": {out.name: sha256(out)},
        }
        (DATA / "SOURCE.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(f"{len(kept)} phrases -> {out}")


def phrases(path: Path = PHRASES_FILE):
    """Rows of phrases.tsv: (the words of the phrase, count in written text, count in spoken text)."""
    if not path.exists():
        return []
    if sha256(path) != json.loads((DATA / "SOURCE.json").read_text())["files"][path.name]:
        raise ValueError("DataSources/mimo-phrases/phrases.tsv changed; make it with scripts/mimo_phrases.py snapshot")
    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not line.startswith("#"):
                words, written, spoken = line.rstrip("\n").split("\t")
                rows.append((tuple(words.split(" ")), int(written), int(spoken)))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="step", required=True)
    p = sub.add_parser("count")
    p.add_argument("lexicon_dir")
    p.add_argument("--exclude", action="append", default=[])
    p.add_argument("--jobs", type=int, default=4)
    p = sub.add_parser("snapshot")
    p.add_argument("--out", type=Path, default=PHRASES_FILE)
    args = parser.parse_args()
    if args.step == "count":
        with ProcessPoolExecutor(args.jobs) as pool:
            jobs = [pool.submit(count_source, name, args.lexicon_dir, args.exclude) for name in sources()]
            for job in jobs:
                name, stats = job.result()
                print(name, json.dumps(stats))
    else:
        snapshot(args.out)


if __name__ == "__main__":
    main()
