"""English words written inside Chinese text, with how often and in which casing.

The pinyin decoder offers a word of this list for letters that are not pinyin
(wozaiyongiphone -> 我在用iPhone). compile-native-lexicon.py reads
DataSources/mimo-latin/latin.tsv and adds the words as rows of the pinyin
image of mimo_base.bin under an upper-case key (IPHONE), which no pinyin
lookup reaches; the decoder looks them up on purpose
(crates/meowpuut-core/src/latin.rs).

  python3 scripts/mimo_latin.py count [--text DIR ...] [--jobs 4]
      Reads the sources of DataSources/mimo-pairs (downloaded by
      scripts/mimo_pairs.py fetch into .build/pair-corpus/dl, and the everyday
      sentences kept in the repo) and the plain text files in each --text DIR, and counts every Latin word that stands
      next to a Han character (one space between them at most): 用iPhone查,
      開 Zoom 會議. A word inside an English phrase or a URL is not counted.
      Spoken text (Tatoeba, Common Voice, the --text files) counts x10, as in
      the pair table. Each word keeps its most frequent spelling, in lower
      case when the text often writes it so (version, team); the key is
      that spelling in lower case without hyphens (Wi-Fi -> wifi). Writes
      DataSources/mimo-latin/latin.tsv and the counts in SOURCE.json.
"""
import argparse
import json
import math
import re
import subprocess
from collections import Counter, defaultdict
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

from mimo_pairs import CACHE, source_texts, sources

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-latin"
SPOKEN_WEIGHT = 10
MIN_COUNT = 5       # weighted count a word needs to be kept
# Short keys meet pinyin letters often (zh, len): two letters need 100, three need 30.
MIN_SHORT = {2: 100, 3: 30}
LIMIT = 12000       # words kept, most frequent first
HAND_COUNT = 150    # weighted count given to a word of the hand list
LOWER_COMMON = 0.2  # lower-case uses per capitalised use, anywhere in lines with Han
LOWER_MIN = 20      # and at least this many lower-case uses (Eason, Plurk keep their capital)
HAN = re.compile(r"[㐀-䶿一-鿿]")
WORD = re.compile(r"[A-Za-z]+(?:-[A-Za-z]+)*")
LINK = re.compile(r"\[\[(?:[^\]|]*\|)?([^\]|]*)\]\]")
TEMPLATE = re.compile(r"\{\{[^{}]*\}\}|-\{[^{}]*\}-|<[^<>]*>")
NEAR = set("0123456789_@/\\.:=&%#+-'$|{}[]")


def is_han(ch: str) -> bool:
    return bool(ch) and bool(HAN.match(ch))


def words_in(line: str, counts: Counter):
    if not HAN.search(line):
        return
    n = len(line)
    for m in WORD.finditer(line):
        s, e = m.span()
        before = line[s - 1] if s else ""
        after = line[e] if e < n else ""
        if before in NEAR or after in NEAR:
            continue
        counts["~" + m.group()] += 1  # any use in the line, in this spelling, for the casing rule
        # A word of an English phrase (said hello world的) is not counted.
        if (before == " " and s >= 2 and line[s - 2].isascii() and line[s - 2].isalpha()) or \
                (after == " " and e + 1 < n and line[e + 1].isascii() and line[e + 1].isalpha()):
            continue
        left = is_han(before) or (before == " " and s >= 2 and is_han(line[s - 2]))
        right = is_han(after) or (after == " " and e + 1 < n and is_han(line[e + 1]))
        if left or right:
            counts[m.group()] += 1


def wiki_lines(path: Path):
    # Article pages only. bzcat is several times faster than Python's bz2; markup is stripped only
    # roughly (links unwrapped, templates and tags removed, table lines skipped): a word must stand
    # next to Han text anyway.
    article = in_text = False
    with subprocess.Popen(["bzcat", str(path)], stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace") as p:
        for line in p.stdout:
            if "<ns>" in line:
                article = line.strip() == "<ns>0</ns>"
            if "<text" in line:
                in_text = article
                line = line[line.find(">", line.find("<text")) + 1:]
            if "</text>" in line:
                line, in_text = line[:line.find("</text>")] if in_text else "", False
            elif not in_text:
                continue
            if line[:1] in "{|!<*#=" or line.startswith("[[File") or line.startswith("[[Category"):
                continue
            line = line.replace("'''", "").replace("''", "")
            for _ in range(2):
                line = TEMPLATE.sub("", line)
            yield LINK.sub(r"\1", line)


def count_file(job):
    name, src = job
    counts = Counter()
    for line in wiki_lines(CACHE / "dl" / src["cache"]) if src["kind"] == "wiki" else source_texts(src):
        words_in(line, counts)
    return name, src["kind"], counts


def count(text_dirs, jobs):
    # Only sentences people wrote as text: the project's own, Tatoeba and Common Voice. The task
    # dialogues are left out: MASSIVE is voice commands in lower case (alexa, iphone, google), and
    # the English in CrossWOZ and KdConv is mostly names of hotels, songs and awards.
    work = [(name, src) for name, src in sources().items() if src["kind"] in ("lines", "tatoeba")]
    for d in text_dirs:
        work += [(f"text:{p.name}", {"path": str(p.resolve()), "kind": "lines"}) for p in sorted(Path(d).glob("*.txt"))]
    forms = defaultdict(Counter)
    anywhere = Counter()
    per_source = Counter()
    with ProcessPoolExecutor(jobs) as pool:
        for name, kind, counts in pool.map(count_file, work):
            weight = 1 if kind == "wiki" else SPOKEN_WEIGHT
            per_source[name.split(":")[0]] += sum(c for w, c in counts.items() if w[0] != "~")
            for word, c in counts.items():
                if word[0] == "~":
                    anywhere[word[1:].replace("-", "")] += c * weight
                    continue
                forms[word.replace("-", "").lower()][word] += c * weight
    rows = []
    for key, spellings in forms.items():
        total = sum(spellings.values())
        if len(key) < 2 or len(key) > 24 or total < max(MIN_COUNT, MIN_SHORT.get(len(key), 0)):
            continue
        best, top = max(spellings.items(), key=lambda kv: (kv[1], kv[0]))
        # Running text writes a common word in lower case (app, email); titles and names capitalise it.
        if spellings.get(key, 0) * 3 >= top:
            best = key
        # Capitalised only in its first letter (Project, not Google) and also common in lower case in the text: lower case.
        elif best[1:].islower() and anywhere[key] >= max(LOWER_MIN, LOWER_COMMON * anywhere[best.replace("-", "")]):
            best = best.lower()
        rows.append((total, key, best))
    # The hand list (Resources/Lexicon/latin.tsv): names and abbreviations in their usual spelling,
    # present even when the counted text lacks them, at HAND_COUNT or their own count if higher.
    listed = {key: (total, best) for total, key, best in rows}
    for line in (ROOT / "Resources/Lexicon/latin.tsv").read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            key = line.replace("-", "").lower()
            listed[key] = (max(listed.get(key, (0, ""))[0], HAND_COUNT), line)
    rows = [(total, key, best) for key, (total, best) in listed.items()]
    rows.sort(key=lambda r: (-r[0], r[1]))
    rows = rows[:LIMIT]
    DATA.mkdir(parents=True, exist_ok=True)
    with open(DATA / "latin.tsv", "w", encoding="utf-8") as f:
        f.write("# key\tspelling\tweighted count (scripts/mimo_latin.py; see SOURCE.json)\n")
        for total, key, best in rows:
            f.write(f"{key}\t{best}\t{total}\n")
    record = json.loads((DATA / "SOURCE.json").read_text()) if (DATA / "SOURCE.json").exists() else {}
    record["counts"] = {"words_kept": len(rows), "min_weighted_count": MIN_COUNT, "min_short": MIN_SHORT, "limit": LIMIT,
                        "latin_tokens_per_source": dict(sorted(per_source.items()))}
    (DATA / "SOURCE.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    print(f"latin.tsv: {len(rows)} words")


def latin_rows():
    """Pinyin lexicon rows for latin.tsv: ((KEY, KEY5, spelling, spelling), weight).

    The key is upper case so that no pinyin lookup finds it; the reading is
    the key with a neutral tone, which keeps the row valid for Lexicon::load.
    Weight on the lexicon's scale (700 per nat): 700 ln(count) + 1000.
    """
    rows = []
    for line in (DATA / "latin.tsv").read_text(encoding="utf-8").splitlines():
        if line.startswith("#"):
            continue
        key, spelling, total = line.split("\t")
        weight = min(9000, round(700 * math.log(int(total))) + 1000)
        rows.append(((key.upper(), key.upper() + "5", spelling, spelling), weight))
    return rows


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("count")
    c.add_argument("--text", action="append", default=[])
    c.add_argument("--jobs", type=int, default=4)
    args = ap.parse_args()
    count(args.text, args.jobs)


if __name__ == "__main__":
    main()
