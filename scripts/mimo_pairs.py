"""Word-pair table: which word follows which, counted in openly licensed text.

The engine adds to each word's lexicon weight how much more (or less) likely
the word is after the word before it, and at the start and end of the input
(crates/meowpuut-core/src/pairs.rs). This script makes that table.

A build only packs: compile-native-lexicon.py calls write_pair_table(), which
reads the committed counts (DataSources/mimo-pairs/counts.txt.xz) and writes
mimo_pairs.bin for the lexicon it has just compiled. The counts are redone by
hand when the sources or the word splitting change:

  python3 scripts/mimo_pairs.py fetch [--seed DIR ...]
      Downloads the sources into .build/pair-corpus/dl and checks SHA-256
      (DataSources/mimo-pairs/SOURCE.json). --seed copies files with the same
      name and checksum from DIR instead of downloading them.
  python3 scripts/mimo_pairs.py count LEXICON_DIR [--exclude FILE.jsonl ...] [--jobs 4]
      Splits every clause into the words of the lexicon in LEXICON_DIR (the
      output of compile-native-lexicon.py) and counts single words and word
      pairs, with the clause start and end, per source. Sentences in the
      --exclude files (test and held-out sets: their Tatoeba ids, and every
      clause that contains one of their texts of 5 or more characters) are
      left out. About 15 minutes with 4 jobs.
  python3 scripts/mimo_pairs.py snapshot LEXICON_DIR [--keep RULE] [--out PATH]
      Merges the counts (spoken text weighted x10) and keeps the pairs RULE
      names: "spoken+wiki3" (every pair seen in spoken text and written pairs
      seen 3+ times; any number), "kl:2000000" (the pairs that change the
      estimate most), "spoken" (spoken text only). Writes the counts file and
      a record of what was counted and left out (counts.json).

The clause-start row and the clause-end column are taken from spoken text
(spoken_boundaries): written text, mostly encyclopedia articles, starts and
ends its clauses with other words than people do in chat. The packer limits
those terms to +-1.5 nats and every other pair score to +-4 nats.

Text is converted to simplified characters with the lexicon's own character
table and split by the most likely path of lexicon words; a word id is the
position of the word's simplified text (lexicon::WordIds).

Scoring, as measured in the research for this table: for word f after word g,
  0.8 * (log P(f|g) - log Pu(f))
P(f|g) is absolute discounting (D = 0.75) interpolated with Pu, and Pu is
0.8 x the counted share of f plus 0.2 x the lexicon's own share. Stored per
pair in steps of 1/64 nat; each left word stores 0.8 * log alpha(g), the score
of a pair that is not stored.
"""
import argparse
import array
import bz2
import functools
import hashlib
import heapq
import html
import io
import json
import lzma
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import zipfile
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-pairs"
COUNTS_FILE = DATA / "counts.txt.xz"
CACHE = ROOT / ".build/pair-corpus"

MAGIC = b"MIMO-PAIR-1\x00"
HEADER_LEN = 64
STEP = 64  # stored values are in 1/64 nat
D, MU, LAM = 0.75, 0.8, 0.8
SPOKEN_WEIGHT = 10
BOUNDARY_PRIOR = 2.0  # spoken clauses a word's clause-end share is pulled towards the average by
START_END_LIMIT = 1.5  # nats, clause start and end terms
PAIR_LIMIT = 4.0  # nats, every other stored pair score and backoff
UNKNOWN_LOGP = -20.0
START, END, UNKNOWN = 0, 1, -1

HAN_RUN = re.compile(r"[㐀-䶿一-鿿]+")
HAN = re.compile(r"[㐀-䶿一-鿿]")
BOUNDARY = re.compile(r"[\s。．！？!?，,；;：:、（）()\[\]【】「」『』《》〈〉“”‘’\"'…—～~·・/\\|=*#<>{}]+")


def sources() -> dict:
    return json.loads((DATA / "SOURCE.json").read_text())["sources"]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def fetch(seeds: list[str]):
    (CACHE / "dl").mkdir(parents=True, exist_ok=True)
    for name, src in sources().items():
        if "path" in src:  # in the repo
            continue
        path = CACHE / "dl" / src["cache"]
        if path.exists() and sha256(path) == src["sha256"]:
            continue
        tmp = path.with_suffix(".part")
        base = src["url"].rsplit("/", 1)[1]
        found = next((Path(folder) / base for seed in seeds for folder, _, files in os.walk(seed)
                      if base in files and sha256(Path(folder) / base) == src["sha256"]), None)
        if found:
            shutil.copyfile(found, tmp)
        else:
            subprocess.run(["curl", "-fL", "--retry", "3", "-o", str(tmp), src["url"]], check=True)
        if sha256(tmp) != src["sha256"]:
            raise SystemExit(f"{name}: SHA-256 differs from SOURCE.json. The file at {src['url']} changed; "
                             "check the new file and its terms, then update SOURCE.json.")
        tmp.replace(path)
        print(f"{name}: {path}")


# --- lexicon --------------------------------------------------------------

def read_rows(lexicon_dir: Path, mode: str):
    """Rows of native-{mode}.tsv as compile-native-lexicon.py writes them: ((bare, reading, hant, hans), weight)."""
    rows = []
    with open(lexicon_dir / f"native-{mode}.tsv", encoding="utf-8") as f:
        next(f)
        for line in f:
            if line.startswith("# context"):
                break
            bare, reading, hant, hans, weight = line.rstrip("\n").split("\t")
            rows.append(((bare, reading, hant, hans), int(weight)))
    return rows


class Lexicon:
    """Word ids, the lexicon's share of each word (log), and the character map to simplified."""

    def __init__(self, pinyin_rows, zhuyin_rows):
        from mimo_binary import forms_fingerprint, word_forms
        self.forms = word_forms(hans for rows in (pinyin_rows, zhuyin_rows) for (_, _, _, hans), _ in rows)
        self.fingerprint = forms_fingerprint(self.forms)
        self.ids = {form: i for i, form in enumerate(self.forms) if i > END}
        # One zhuyin row per reading of a word: the share of a word sums its readings.
        sums = {}
        for (_, _, _, hans), weight in zhuyin_rows:
            sums.setdefault(hans, []).append(weight / 700.0)
        top = max(max(v) for v in sums.values())
        z = top + math.log(sum(math.exp(w - top) for v in sums.values() for w in v))
        floor = min(min(v) for v in sums.values()) - z
        self.logp = [0.0, 0.0] + [floor] * (len(self.forms) - 2)
        for hans, ws in sums.items():
            m = max(ws)
            self.logp[self.ids[hans]] = m + math.log(sum(math.exp(w - m) for w in ws)) - z
        best = {}
        for (_, _, hant, hans), weight in zhuyin_rows:
            if len(hant) == 1 and len(hans) == 1 and HAN.match(hant) and best.get(hant, ("", -1))[1] < weight:
                best[hant] = (hans, weight)
        self.to_simplified = {ord(t): s for t, (s, _) in best.items() if s != t}
        self.longest = max(len(f) for f in self.ids)
        self.prefixes = {f[:k] for f in self.ids for k in range(1, len(f) + 1)}

    def split(self, run: str) -> list[int]:
        """Most likely split of a run of Han characters into word ids (UNKNOWN for a character the lexicon lacks)."""
        n = len(run)
        best = [-1e18] * (n + 1)
        back = [0] * (n + 1)
        best[0] = 0.0
        ids, logp, prefixes = self.ids, self.logp, self.prefixes
        for i in range(n):
            score = best[i]
            if run[i] not in ids and score + UNKNOWN_LOGP > best[i + 1]:
                best[i + 1] = score + UNKNOWN_LOGP
                back[i + 1] = i
            for j in range(i + 1, min(n, i + self.longest) + 1):
                word = run[i:j]
                if word not in prefixes:
                    break
                k = ids.get(word)
                if k is not None and score + logp[k] > best[j]:
                    best[j] = score + logp[k]
                    back[j] = i
        out = []
        j = n
        while j > 0:
            i = back[j]
            out.append(ids.get(run[i:j], UNKNOWN))
            j = i
        out.reverse()
        return out


def lexicon_from_dir(lexicon_dir: str) -> Lexicon:
    path = Path(lexicon_dir)
    return Lexicon(read_rows(path, "pinyin"), read_rows(path, "zhuyin"))


# --- text -----------------------------------------------------------------

RE_COMMENT = re.compile(r"<!--.*?-->", re.S)
RE_REF = re.compile(r"<ref[^>/]*/>|<ref[^>]*>.*?</ref>", re.S | re.I)
RE_TAGBLOCK = re.compile(r"<(math|code|pre|syntaxhighlight|gallery|timeline|score|chem)[^>]*>.*?</\1>", re.S | re.I)
RE_TAG = re.compile(r"<[^>]{1,200}>")
RE_TEMPLATE = re.compile(r"\{\{[^{}]*\}\}")
RE_TABLE = re.compile(r"\{\|.*?\|\}", re.S)
RE_CONVERT = re.compile(r"-\{([^{}]*)\}-")
RE_FILE = re.compile(r"\[\[(?:File|Image|文件|檔案|档案|图像|圖像|Category|分类|分類|Wikipedia|WP|Help|Template|Media):"
                     r"[^\[\]]*(?:\[\[[^\[\]]*\]\][^\[\]]*)*\]\]", re.I)
RE_LINK = re.compile(r"\[\[([^\[\]|]*\|)?([^\[\]]*)\]\]")
RE_EXTERNAL = re.compile(r"\[(?:https?:)?//[^\s\]]+\s*([^\]]*)\]")


def _convert(m):
    body = m.group(1)
    if ":" in body and (";" in body or body.startswith("zh")):
        first = body.split(";")[0]
        return first.split(":", 1)[1] if ":" in first else first
    return body


def plain_wikitext(text: str) -> str:
    """Article text without markup, templates, tables, references, headings and list markers."""
    text = RE_COMMENT.sub("", text)
    text = RE_REF.sub("", text)
    text = RE_TAGBLOCK.sub("", text)
    text = RE_CONVERT.sub(_convert, text)
    for _ in range(6):
        new = RE_TEMPLATE.sub("", text)
        if new == text:
            break
        text = new
    text = RE_TABLE.sub("", text)
    text = RE_FILE.sub("", text)
    text = RE_LINK.sub(lambda m: m.group(2), text)
    text = RE_EXTERNAL.sub(lambda m: m.group(1), text)
    text = RE_TAG.sub("", text)
    lines = (line.strip() for line in text.split("\n"))
    return "\n".join(s for s in lines if s and s[0] not in "=|!{}" and not s.startswith("[["))


def wiki_pages(path: Path):
    """Main-namespace article texts of a MediaWiki XML dump (.bz2), redirects left out."""
    buf, in_text, main = [], False, False
    with bz2.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            if not in_text:
                if "<ns>" in line:
                    main = line.strip() == "<ns>0</ns>"
                    continue
                i = line.find("<text")
                if i < 0:
                    continue
                j = line.find(">", i)
                if line[j - 1] == "/":
                    continue
                buf = [line[j + 1:]]
                in_text = True
                if "</text>" not in buf[0]:
                    continue
            else:
                buf.append(line)
                if "</text>" not in line:
                    continue
            in_text = False
            text = "".join(buf)
            text = text[:text.rfind("</text>")]
            if main and not text[:9].upper().startswith("#REDIRECT") and not text.startswith("#重定向"):
                yield plain_wikitext(html.unescape(text))


def tatoeba_sentences(path: Path, excluded_ids: set):
    with bz2.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            if len(p) >= 3 and p[1] == "cmn" and p[0] not in excluded_ids:
                yield p[2]


def lines(path: Path):
    with (lzma.open(path, "rt", encoding="utf-8") if path.suffix == ".xz" else open(path, encoding="utf-8")) as f:
        yield from f


def dialogue_turns(path: Path, kind: str, member: str = ""):
    """What people say in the task dialogues of CrossWOZ (a zip with one JSON file) and KdConv (JSON),
    and the utterances of MASSIVE (JSON lines inside a tar.gz; `member` names the file)."""
    if kind == "massive":
        with tarfile.open(path) as archive:
            for line in io.TextIOWrapper(archive.extractfile(member), encoding="utf-8"):
                yield json.loads(line)["utt"]
    elif kind == "crosswoz":
        with zipfile.ZipFile(path) as archive:
            for dialogue in json.load(archive.open(archive.namelist()[0])).values():
                for turn in dialogue["messages"]:
                    yield turn["content"]
    else:
        for dialogue in json.loads(path.read_text(encoding="utf-8")):
            for turn in dialogue["messages"]:
                yield turn["message"]


def source_texts(src: dict, excluded_tatoeba_ids=frozenset()):
    """The sentences (or, for a wiki, the articles) of one source of SOURCE.json."""
    path = ROOT / src["path"] if "path" in src else CACHE / "dl" / src["cache"]
    if src["kind"] == "wiki":
        return wiki_pages(path)
    if src["kind"] == "tatoeba":
        return tatoeba_sentences(path, excluded_tatoeba_ids)
    if src["kind"] in ("crosswoz", "kdconv", "massive"):
        return dialogue_turns(path, src["kind"], src.get("member", ""))
    return lines(path)


def exclusions(files: list[str], to_simplified: dict):
    """Texts (simplified Han characters, 5 or more) and Tatoeba ids from test and held-out sets."""
    texts, ids = set(), set()
    for path in files:
        for line in open(path, encoding="utf-8"):
            item = json.loads(line)
            for key in ("expected", "text", "simp"):
                if isinstance(item.get(key), str):
                    texts.add(item[key])
            texts.update(t for t in item.get("accept") or [] if isinstance(t, str))
            if item.get("tatoeba"):
                ids.add(str(item["tatoeba"]))
            ids.update(re.findall(r"^tat-(\d+)$", str(item.get("id", ""))))
            ids.update(re.findall(r"Tatoeba sentence #(\d+)", str(item.get("source", ""))))
    han = {"".join(HAN_RUN.findall(t.translate(to_simplified))) for t in texts}
    return {t for t in han if len(t) >= 5}, ids


# --- counting -------------------------------------------------------------

class Counts:
    """Single-word and pair counts of one source; pairs keyed by (left id << 20) | right id."""

    FLUSH = 4_000_000

    def __init__(self, n_forms):
        self.uni = array.array("Q", bytes(8 * n_forms))
        self.pairs = Counter()
        self.runs = []

    def add_clause(self, word_runs, starts, ends):
        for k, ids in enumerate(word_runs):
            for w in ids:
                if w != UNKNOWN:
                    self.uni[w] += 1
            if k == 0 and starts:
                ids.insert(0, START)
                self.uni[START] += 1
            if k == len(word_runs) - 1 and ends:
                ids.append(END)
                self.uni[END] += 1
            pairs = self.pairs
            for a, b in zip(ids, ids[1:]):
                if a != UNKNOWN and b != UNKNOWN:
                    pairs[(a << 20) | b] += 1
        if len(self.pairs) > self.FLUSH:
            self._flush()

    def _flush(self):
        codes = sorted(self.pairs)
        self.runs.append((array.array("Q", codes), array.array("Q", (self.pairs[c] for c in codes))))
        self.pairs = Counter()

    def finish(self):
        self._flush()
        return merge_runs(self.runs)


def merge_runs(runs):
    """Sorted (codes, counts) runs into one, summing equal codes."""
    codes, counts = array.array("Q"), array.array("Q")
    last = -1
    for code, n in heapq.merge(*(zip(c, n) for c, n in runs)):
        if code == last:
            counts[-1] += n
        else:
            codes.append(code)
            counts.append(n)
            last = code
    return codes, counts


@functools.lru_cache(maxsize=None)
def blocked_terms():
    """Pattern of Resources/Lexicon/blocked-terms.txt: politics, words that put down a group, abuse."""
    text = (ROOT / "Resources/Lexicon/blocked-terms.txt").read_text(encoding="utf-8")
    terms = [term for line in text.splitlines() if not line.startswith("#") for term in line.split()]
    return re.compile("|".join(sorted(map(re.escape, terms), key=len, reverse=True)))


def clauses(name: str, lex: Lexicon, exclude: list[str], stats: Counter):
    """Clauses of one source in simplified characters, each with its runs of Han characters.
    Left out: clauses of the test and held-out sets (--exclude), and every sentence with a blocked
    term (the whole line of a source of sentences, the clause in an article).
    Returns the iterator and what was excluded."""
    src = sources()[name]
    texts, ids = exclusions(exclude, lex.to_simplified)
    head4 = {t[:4] for t in texts}
    lengths = sorted({len(t) for t in texts})

    def excluded(han: str) -> bool:
        for i in range(len(han) - 4):
            if han[i:i + 4] in head4 and any(han[i:i + n] in texts for n in lengths):
                return True
        return False

    documents = source_texts(src, ids)
    blocked, sentences = blocked_terms(), src["kind"] != "wiki"

    def walk():
        for document in documents:
            document = document.translate(lex.to_simplified)
            if sentences and blocked.search(document):
                stats["sentences_blocked"] += 1
                continue
            for segment in BOUNDARY.split(document):
                runs = HAN_RUN.findall(segment)
                han = "".join(runs)
                if len(han) < 2:
                    continue
                if not sentences and blocked.search(han):
                    stats["clauses_blocked"] += 1
                    continue
                stats["clauses"] += 1
                if excluded(han):
                    stats["clauses_excluded"] += 1
                    continue
                stats["characters"] += len(han)
                yield segment, runs
    return walk(), texts, ids


def count_source(name: str, lexicon_dir: str, exclude: list[str]):
    lex = lexicon_from_dir(lexicon_dir)
    counts = Counts(len(lex.forms))
    stats = Counter()
    found, texts, ids = clauses(name, lex, exclude, stats)
    for segment, runs in found:
        counts.add_clause([lex.split(r) for r in runs], bool(HAN.match(segment[:1])), bool(HAN.match(segment[-1:])))
    codes, pair_counts = counts.finish()
    out = CACHE / "counts"
    out.mkdir(parents=True, exist_ok=True)
    with open(out / f"{name}.bin", "wb") as f:
        counts.uni.tofile(f)
        codes.tofile(f)
        pair_counts.tofile(f)
    stats.update(pairs=len(codes), pair_tokens=sum(pair_counts), words=sum(counts.uni) - counts.uni[START] - counts.uni[END])
    meta = {"source": name, "forms": len(lex.forms), "fingerprint": lex.fingerprint, "stats": dict(stats),
            "excluded_texts": len(texts), "excluded_tatoeba_ids": len(ids),
            "exclude_files": {Path(p).name: sha256(Path(p)) for p in exclude}}
    (out / f"{name}.json").write_text(json.dumps(meta, indent=1))
    return name, dict(stats)


def load_counts(name: str, lex: Lexicon):
    meta = json.loads((CACHE / "counts" / f"{name}.json").read_text())
    if meta["fingerprint"] != lex.fingerprint:
        raise SystemExit(f"counts of {name} were made with another lexicon; run count again")
    n, p = meta["forms"], meta["stats"]["pairs"]
    data = (CACHE / "counts" / f"{name}.bin").read_bytes()
    uni, codes, counts = array.array("Q"), array.array("Q"), array.array("Q")
    uni.frombytes(data[:8 * n])
    codes.frombytes(data[8 * n:8 * (n + p)])
    counts.frombytes(data[8 * (n + p):])
    return meta, uni, codes, counts


# --- snapshot -------------------------------------------------------------

def snapshot(lexicon_dir: str, keep: str, out: Path):
    lex = lexicon_from_dir(lexicon_dir)
    groups = {"written": [], "spoken": []}
    metas = {}
    for name, src in sources().items():
        meta, uni, codes, counts = load_counts(name, lex)
        metas[name] = meta
        groups[src["group"]].append((uni, codes, counts))
    n = len(lex.forms)
    weights = {"written": 0 if keep == "spoken" else 1, "spoken": SPOKEN_WEIGHT}
    uni = [0] * n
    for group, items in groups.items():
        for u, _, _ in items:
            for i in range(n):
                uni[i] += weights[group] * u[i]
    written = merge_runs([(c, k) for _, c, k in groups["written"]])
    spoken = merge_runs([(c, k) for _, c, k in groups["spoken"]])
    codes, wc, sc = array.array("Q"), array.array("Q"), array.array("Q")
    for code, w, s in merge_groups(written, spoken):
        codes.append(code)
        wc.append(w)
        sc.append(s)
    weighted = array.array("Q", (weights["written"] * w + weights["spoken"] * s for w, s in zip(wc, sc)))
    spoken_boundaries(codes, sc, weighted, uni)
    spoken_uni = [sum(u[i] for u, _, _ in groups["spoken"]) for i in range(n)]
    context = [0] * n
    for code, c in zip(codes, weighted):
        context[code >> 20] += c
    if keep == "spoken":
        kept = [i for i in range(len(codes)) if sc[i]]
    elif keep.startswith("spoken+wiki"):
        least = int(keep[len("spoken+wiki"):])
        kept = [i for i in range(len(codes)) if sc[i] or wc[i] >= least]
    elif keep.startswith("kl:"):
        kept = relevant_pairs(codes, weighted, uni, context, lex, int(keep[3:]))
    else:
        raise SystemExit(f"unknown --keep rule {keep}")
    with lzma.open(out, "wt", encoding="utf-8", preset=6) as f:
        f.write(f"# mimo-pairs counts: rule {keep}, spoken text x{SPOKEN_WEIGHT}; "
                "U form count followers spoken-count, P left right count ...\n")
        for i in range(n):
            if uni[i] or context[i] or spoken_uni[i]:
                f.write(f"U\t{lex.forms[i]}\t{uni[i]}\t{context[i]}\t{spoken_uni[i]}\n")
        left, line = -1, []
        for i in kept:
            g, w = codes[i] >> 20, lex.forms[codes[i] & 0xFFFFF]
            if g != left:
                if line:
                    f.write("\t".join(line) + "\n")
                left, line = g, ["P", lex.forms[g]]
            line.append(f"{w} {weighted[i]}")
        if line:
            f.write("\t".join(line) + "\n")
    first = next(iter(metas.values()))
    record = {"rule": keep, "spoken_weight": SPOKEN_WEIGHT, "pairs_counted": len(codes), "pairs_kept": len(kept),
              "bytes": out.stat().st_size, "exclude_files": first["exclude_files"],
              "excluded_texts": first["excluded_texts"], "excluded_tatoeba_ids": first["excluded_tatoeba_ids"],
              "sources": {name: m["stats"] for name, m in metas.items()}}
    out.with_name(out.name.split(".")[0] + ".json").write_text(json.dumps(record, ensure_ascii=False, indent=1) + "\n")
    if out == COUNTS_FILE:
        manifest = json.loads((DATA / "SOURCE.json").read_text())
        manifest["files"] = {name: sha256(DATA / name) for name in ("counts.txt.xz", "counts.json")}
        (DATA / "SOURCE.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(record, ensure_ascii=False, indent=1))


def spoken_boundaries(codes, sc, weighted, uni):
    """Clause-start row and clause-end column from spoken text, in place.

    The start row keeps only its spoken counts. A word's clause-end count is set so that its
    share of the word's followers is its spoken share: (spoken ends + 2 x average) / (spoken
    followers + 2), the average being the share over all spoken words. A word never seen in
    spoken text gets the average."""
    n = len(uni)
    spoken_context, rest = [0] * n, [0] * n
    for code, s, c in zip(codes, sc, weighted):
        spoken_context[code >> 20] += s
        if code & 0xFFFFF != END:
            rest[code >> 20] += c
    ends = [i for i, code in enumerate(codes) if code & 0xFFFFF == END and code >> 20 > END]
    average = sum(sc[i] for i in ends) / max(1, sum(spoken_context[2:]))
    uni[START] = uni[END] = 0
    for i, code in enumerate(codes):
        if code >> 20 != START:
            break
        weighted[i] = SPOKEN_WEIGHT * sc[i]
        uni[START] += weighted[i]
    for i in ends:
        g = codes[i] >> 20
        share = (sc[i] + BOUNDARY_PRIOR * average) / (spoken_context[g] + BOUNDARY_PRIOR)
        weighted[i] = round(share / (1 - share) * rest[g])
        uni[END] += weighted[i]


def merge_groups(a, b):
    """Two sorted (codes, counts) into (code, count in a, count in b)."""
    i = j = 0
    ac, an = a
    bc, bn = b
    while i < len(ac) or j < len(bc):
        if j == len(bc) or (i < len(ac) and ac[i] < bc[j]):
            yield ac[i], an[i], 0
            i += 1
        elif i == len(ac) or bc[j] < ac[i]:
            yield bc[j], 0, bn[j]
            j += 1
        else:
            yield ac[i], an[i], bn[j]
            i += 1
            j += 1


def unigram_share(uni, lex: Lexicon):
    """Pu: 0.8 x counted share + 0.2 x lexicon share; tiny for the clause start, no lexicon share for the end."""
    total = sum(uni[2:]) or 1
    norm = math.log(sum(math.exp(v) for v in lex.logp[2:]))
    pu = [MU * uni[i] / total + (1 - MU) * math.exp(lex.logp[i] - norm) for i in range(len(uni))]
    pu[START] = 1e-12
    pu[END] = max(MU * uni[END] / total, 1e-12)
    return pu


def relevant_pairs(codes, weighted, uni, context, lex, top):
    """Indices of the `top` pairs whose estimate differs most from their backoff, weighted by count."""
    pu = unigram_share(uni, lex)
    mass = [0.0] * len(uni)
    for code, c in zip(codes, weighted):
        mass[code >> 20] += max(c - D, 0.0)
    alpha = [1.0 - mass[g] / max(context[g], 1) for g in range(len(uni))]
    scores = array.array("d")
    for code, c in zip(codes, weighted):
        g, f = code >> 20, code & 0xFFFFF
        back = max(alpha[g], 1e-6) * pu[f]
        scores.append(c * abs(math.log(max(c - D, 0.0) / max(context[g], 1) + back) - math.log(back)))
    lo, hi = 0.0, max(scores)
    for _ in range(50):
        mid = (lo + hi) / 2
        if sum(1 for s in scores if s >= mid) > top:
            lo = mid
        else:
            hi = mid
    return [i for i, s in enumerate(scores) if s >= hi]


# --- packing (build time) -------------------------------------------------

def write_pair_table(output_dir: Path, pinyin_rows, zhuyin_rows, counts_file: Path = COUNTS_FILE) -> dict:
    """Packs the counts for the lexicon just compiled into output_dir/mimo_pairs.bin.

    Layout (little-endian, read in place by crates/meowpuut-core/src/pairs.rs):
      header 64 bytes: magic MIMO-PAIR-1, word count n (u32), pair count p (u32), u32 0,
                       fingerprint of the lexicon's word list (u64), zeros
      offsets (n + 1) x u32: followers of left word g are pairs[offsets[g]..offsets[g + 1]]
      backoff n x i16: 0.8 * log alpha(g), the score of a pair that is not stored
      padding to 4 bytes
      pairs   p x u32: (right id << 12) | 12-bit signed score, sorted by right id per left word
    Words the lexicon no longer has are dropped with their pairs.
    """
    if counts_file == COUNTS_FILE and sha256(counts_file) != json.loads((DATA / "SOURCE.json").read_text())["files"]["counts.txt.xz"]:
        raise ValueError("DataSources/mimo-pairs/counts.txt.xz changed; make it with scripts/mimo_pairs.py snapshot")
    lex = Lexicon(pinyin_rows, zhuyin_rows)
    n = len(lex.forms)
    uni, context = [0] * n, [0] * n
    pairs = {}
    with lzma.open(counts_file, "rt", encoding="utf-8") as f:
        for line in f:
            fields = line.rstrip("\n").split("\t")
            if fields[0] == "U":
                i = lex.ids.get(fields[1], START if fields[1] == "<s>" else END if fields[1] == "</s>" else None)
                if i is not None:
                    uni[i], context[i] = int(fields[2]), int(fields[3])
            elif fields[0] == "P":
                g = marker_or_id(lex, fields[1])
                if g is None:
                    continue
                for item in fields[2:]:
                    word, c = item.rsplit(" ", 1)
                    w = marker_or_id(lex, word)
                    if w is not None:
                        pairs[(g << 20) | w] = int(c)
    pu = unigram_share(uni, lex)
    mass = [0.0] * n
    for code, c in pairs.items():
        mass[code >> 20] += max(c - D, 0.0)
    alpha = [1.0 - mass[g] / context[g] if context[g] else 1.0 for g in range(n)]
    values = {}
    for code, c in pairs.items():
        g, w = code >> 20, code & 0xFFFFF
        p = max(c - D, 0.0) / max(context[g], 1) + alpha[g] * pu[w]
        limit = (START_END_LIMIT if g == START or w == END else PAIR_LIMIT) * STEP
        values[code] = round(max(-limit, min(limit, LAM * (math.log(p) - math.log(pu[w])) * STEP)))
    limits = [START_END_LIMIT * STEP] + [PAIR_LIMIT * STEP] * (n - 1)
    backoff = [round(max(-limit, LAM * math.log(max(a, 1e-30)) * STEP)) for a, limit in zip(alpha, limits)]
    cap_variants(values, default_forms(lex), backoff)
    codes = sorted(values)
    offsets = array.array("I", [0] * (n + 1))
    packed = array.array("I")
    for code in codes:
        offsets[(code >> 20) + 1] += 1
        packed.append(((code & 0xFFFFF) << 12) | (max(-2048, min(2047, values[code])) & 0xFFF))
    for g in range(n):
        offsets[g + 1] += offsets[g]
    backoff = array.array("h", backoff)
    if sys.byteorder != "little":
        for arr in (offsets, packed, backoff):
            arr.byteswap()
    header = MAGIC + struct.pack("<3IQ", n, len(packed), 0, lex.fingerprint)
    body = header + bytes(HEADER_LEN - len(header)) + offsets.tobytes() + backoff.tobytes()
    body += bytes(-len(body) % 4) + packed.tobytes()
    (Path(output_dir) / "mimo_pairs.bin").write_bytes(body)
    return {"pair_words": n, "pairs": len(packed), "mimo_pairs_bin_bytes": len(body)}


@functools.lru_cache(maxsize=None)
def default_swaps() -> tuple:
    """(variant, default) characters of Resources/Lexicon/pair-defaults.tsv."""
    return tuple(tuple(line.split("|")[:2]) for line in (ROOT / "Resources/Lexicon/pair-defaults.tsv").read_text(encoding="utf-8").splitlines()
                 if line and not line.startswith("#"))


def default_text(text: str):
    """The default form of a variant word (她的 -> 他的), or None."""
    for variant, default in default_swaps():
        if variant in text:
            return text.replace(variant, default)
    return None


def default_forms(lex: Lexicon) -> dict:
    """Word id -> id of its default form, where the default text is also a word."""
    defaults = {}
    for form, i in lex.ids.items():
        j = lex.ids.get(default_text(form) or "")
        if j is not None:
            defaults[i] = j
    return defaults


def cap_variants(values, defaults, backoff):
    """A variant form never scores above its default form, neither after a word nor before one (or
    the clause end): one neighbouring word is not evidence enough for 她 or 它 over 他, or for 嘛 over 吗."""
    for g, d in defaults.items():
        backoff[g] = min(backoff[g], backoff[d])
    for code, value in values.items():
        g, w = code >> 20, code & 0xFFFFF
        if w in defaults:
            value = min(value, values.get((g << 20) | defaults[w], backoff[g]))
        if g in defaults:
            value = min(value, values.get((defaults[g] << 20) | w, backoff[defaults[g]]))
        values[code] = value


@functools.lru_cache(maxsize=None)
def counted_words(counts_file: Path = COUNTS_FILE) -> dict:
    """Word (simplified text) -> (count in written text, count in spoken text): the U rows."""
    counts = {}
    with lzma.open(counts_file, "rt", encoding="utf-8") as f:
        for line in f:
            if line.startswith("P"):
                break
            fields = line.rstrip("\n").split("\t")
            if fields[0] == "U":
                counts[fields[1]] = (int(fields[2]) - SPOKEN_WEIGHT * int(fields[4]), int(fields[4]))
    return counts


def word_counts() -> dict:
    """How often each word was counted in the sources (spoken text x10)."""
    return {word: n + SPOKEN_WEIGHT * spoken for word, (n, spoken) in counted_words().items() if n or spoken}


def spoken_counts() -> dict:
    """How often each word was counted in the spoken sources."""
    return {word: n for word, (_, n) in counted_words().items() if n}


def marker_or_id(lex: Lexicon, text: str):
    if text == "<s>":
        return START
    if text == "</s>":
        return END
    return lex.ids.get(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="step", required=True)
    p = sub.add_parser("fetch")
    p.add_argument("--seed", action="append", default=[])
    p = sub.add_parser("count")
    p.add_argument("lexicon_dir")
    p.add_argument("--exclude", action="append", default=[])
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("--only", action="append", help="count only these sources")
    p = sub.add_parser("snapshot")
    p.add_argument("lexicon_dir")
    p.add_argument("--keep", default="kl:2000000")
    p.add_argument("--out", type=Path, default=COUNTS_FILE)
    args = parser.parse_args()
    if args.step == "fetch":
        fetch(args.seed)
    elif args.step == "count":
        if not args.exclude:
            print("warning: no --exclude files; test and held-out sentences stay in the counts", file=sys.stderr)
        names = args.only or list(sources())
        with ProcessPoolExecutor(args.jobs) as pool:
            jobs = [pool.submit(count_source, name, args.lexicon_dir, args.exclude) for name in names]
            for job in jobs:
                name, stats = job.result()
                print(name, json.dumps(stats))
    else:
        snapshot(args.lexicon_dir, args.keep, args.out)


if __name__ == "__main__":
    main()
