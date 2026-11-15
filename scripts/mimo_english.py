"""English word list and word pairs for the English keyboard (nine-key, completion, next word).

Reads the sources of DataSources/mimo-english/SOURCE.json and writes, deterministically:

  DataSources/mimo-english/english.tsv.xz  key, display spelling, weighted count, flags
  DataSources/mimo-english/pairs.tsv.xz    left, right, weighted count (display spellings; <s> and </s> are
                                           the sentence start and end)

  python3 -I scripts/mimo_english.py fetch [--seed DIR ...]
      Downloads the sources into .build/english-corpus/dl and checks SHA-256 (SOURCE.json). --seed copies
      files with the same name and checksum from DIR instead of downloading them.
  python3 -I scripts/mimo_english.py count [--exclude-ids FILE] [--text DIR ...]
      Makes the two files and records the counts and the SHA-256 of the files in SOURCE.json.
      --exclude-ids: Tatoeba sentence ids (one per line) left out of the counts (held-out test sentences).
      --text: plain text files (*.txt, one sentence per line, spoken weight) counted as well.
      A few minutes; about 3 GB of memory.
  python3 -I scripts/mimo_english.py relist
      Applies the [en] list as it is now to the two files without counting again: its words get the
      flag nosuggest and the pairs with one of them are left out (as scripts/mimo_en_binary.py does at
      build time). Records the SHA-256 of the files in SOURCE.json.

Words. A word is a spelling of the American list of SCOWL/ESDB (size 70 or smaller; the file data/scowl-pre.txt
of the pinned commit, read here, not built with the SCOWL tools), or a spelling of DataSources/mimo-latin or
Resources/Lexicon/latin.tsv. A British-only spelling, a word with a digit or a space, a prefix and a Roman
numeral are not kept. The key is the lower-case spelling without accents, apostrophes and hyphens
(Wi-Fi -> wifi, don't -> dont); words that share a key (its, it's) are separate rows. The spelling is the one
of the list; a word the text writes in lower case at least a third as often as in capitals (OK, Internet)
is shown in lower case. Casing is read from the middle of sentences only: a capital at the start of a
sentence says nothing.

Counts. Tatoeba (English) and the Common Voice sentence collector are spoken text, counted x10 (SPOKEN_WEIGHT),
as in scripts/mimo_pairs.py. A sentence with a word of the [en] section of Resources/Lexicon/blocked-terms.txt (or a word SCOWL
marks offensive-1, offensive-2, vulgar-1) is not counted at all. Tatoeba is full of Tom and Mary: the count of
a first name or surname of SCOWL in Tatoeba is limited to the Tatoeba count of the 2,000th word, and the pairs
that contain it are scaled by the same factor. A word of mimo-latin that the text counts less often keeps the
count it has there. Flags:

  proper     capital first letter only (London, Monday, Tom)
  acronym    all capitals, two letters or more (NASA, OK)
  mixed      capitals inside the word (iPhone, YouTube)
  in-chinese the word is in mimo-latin or in Resources/Lexicon/latin.tsv (written inside Chinese text)
  nosuggest  in the blocked list: never suggested, still typed in full

Pairs. Neighbouring words of a sentence (split at . ! ? ; a comma does not split). A word that is not a row
breaks the chain. Pairs seen twice or more are ranked by how much they tell (count x ln(count x N / (left x
right))) and the best PAIR_TARGET are kept.
"""
import argparse
import bz2
import hashlib
import json
import lzma
import math
import os
import re
import shutil
import subprocess
import unicodedata
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-english"
CACHE = ROOT / ".build/english-corpus"
SPOKEN_WEIGHT = 10
SCOWL_SIZE = 70         # words of SCOWL size 70 or smaller
LIMIT = 60000           # rows kept, most frequent first
LATIN_ONLY_MIN = 30     # a word of mimo-latin that SCOWL lacks needs this count (3 uses in spoken text)
HAND_COUNT = 150        # weighted count of a word of the hand list that mimo-latin does not count
MIN_COUNT = 20          # weighted count (2 uses) a row needs, unless SCOWL calls it common (size <= SMALL_SIZE) or it is in mimo-latin
SMALL_SIZE = 50
CAP_RANK = 2000         # a first name or surname counts at most as often (in Tatoeba) as the word of this rank
LOWER_SHARE = 3         # lower case at least 1/3 as often as the capitalised spelling -> lower case
FORM_SHARE = 5          # a second spelling of a key (us/US) is a row if it is at least 1/5 of the first
MIN_PAIR = 2            # uses a pair needs
PAIR_TARGET = 900000    # pairs kept (the spec's 1.2 million is for 80,000 words)
START, END = "<s>", "</s>"
NAME_CLASSES = {"person", "surname", "name", "name?", "place", "upper", "upper?"}
FIXED = {"I", "I'm", "I'll", "I'd", "I've", "I'd've"}
WORD = re.compile(r"\w+(?:['’\-]\w+)*")
SENTENCE_END = re.compile(r"(?<![A-Za-z])((?:[Mm]r|[Mm]rs|[Mm]s|[Dd]r|[Ss]t|[Jj]r|[Ss]r|[Pp]rof)\.)|[.!?;…]+(?=\s|$)|[\r\n]+")
VARIANT_TOKEN = re.compile(r"^(?:[ABZCD_][.=?vV~@x\-]?(?:\{\d+\})?|[=+])$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def record() -> dict:
    return json.loads((DATA / "SOURCE.json").read_text(encoding="utf-8"))


def fetch(seeds):
    (CACHE / "dl").mkdir(parents=True, exist_ok=True)
    for name, src in record()["sources"].items():
        path = CACHE / "dl" / src["cache"]
        if path.exists() and sha256(path) == src["sha256"]:
            continue
        tmp = path.with_suffix(path.suffix + ".part")
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


def dl(name: str) -> Path:
    return CACHE / "dl" / record()["sources"][name]["cache"]


# --- keys ----------------------------------------------------------------------------------------

def deaccent(s: str) -> str:
    return "".join(c for c in unicodedata.normalize("NFD", s) if not unicodedata.combining(c))


def make_key(form: str) -> str:
    return re.sub(r"['’\-]", "", deaccent(form)).lower()


def plain_form(token: str) -> str:
    """A token as the blocked list spells it: lower case, no accents, no 's, no hyphens (who're stays who're)."""
    return re.sub(r"['’]s$", "", deaccent(token).lower()).replace("-", "")


def casing(form: str) -> set:
    flags = set()
    letters = [c for c in form if c.isalpha()]
    if form in FIXED or not letters:
        return flags
    rest = [c for c in form[1:] if c.isalpha()]
    if len(letters) >= 2 and all(c.isupper() for c in letters):
        flags.add("acronym")
    elif form[0].isupper() and not any(c.isupper() for c in rest):
        flags.add("proper")
    elif any(c.isupper() for c in rest):
        flags.add("mixed")
    return flags


# --- SCOWL ---------------------------------------------------------------------------------------

def variant_us(info: str) -> bool:
    """A line's variant information: is the lemma an American (or neutral) spelling?"""
    tokens = info.split()
    if not tokens:
        return True
    return any(t[0] in "A_" and (len(t) == 1 or t[1] in ".=?v")
               for t in tokens if VARIANT_TOKEN.match(t) and t[0] not in "=+")


def split_entries(text: str):
    out, depth, cur = [], 0, []
    for ch in text:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    out.append("".join(cur).strip())
    return [e for e in out if e]


def derived_forms(entry: str):
    """The spellings of one entry of the derived list: 'aahed-' or '(alighted | V: alit)'."""
    if entry.startswith("("):
        alternatives = [a.strip() for a in entry[1:entry.rfind(")")].split("|")]
    else:
        alternatives = [entry]
    out = []
    for alt in alternatives:
        info, _, word = alt.rpartition(": ")
        if info and not variant_us(info):
            continue
        word = word.strip()
        if word and word[-1] in "@~!†":
            continue
        out.append(word.rstrip("*-") if len(word) > 1 and word[-1] in "*-" else word)
    return out


def scowl_forms(path: Path, exclude_path: Path):
    """Spellings of the American list (size <= SCOWL_SIZE) -> [size, is a name], and the
    keys SCOWL marks offensive-1, offensive-2 or vulgar-1."""
    bumped = {}
    lemma = None
    for line in exclude_path.read_text(encoding="utf-8").splitlines():
        m = re.match(r"^= (\d+) \[-\]", line)
        if m and lemma:
            bumped[lemma] = int(m.group(1))
        elif line and not line.startswith(("#", "=")):
            lemma = line.split(" <")[0]
    forms = {}
    offensive = set()
    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.rstrip("\n")
            if not line or line[0] == "#":
                continue
            line = re.sub(r" #.*$", "", line)
            parts = line.split(": ")
            m = re.match(r"^(\d+)", parts[0])
            if not m or len(parts) < 2:
                continue
            size = int(m.group(1))
            rest = parts[1:]
            variant = ""
            if len(rest) >= 2 and rest[0].strip() and all(VARIANT_TOKEN.match(t) for t in rest[0].split()):
                variant, rest = rest[0], rest[1:]
            lemma_info = rest[0]
            entries = split_entries(": ".join(rest[1:])) if len(rest) > 1 else []
            pos = re.search(r"<([a-z_]*)(?:/([a-z?]*))?>", lemma_info)
            usage = re.search(r"\(([a-z0-9\-]+)\)", lemma_info)
            word = re.sub(r" ?[<{(].*$", "", lemma_info).strip()
            posname = pos.group(1) if pos else ""
            posclass = (pos.group(2) or "") if pos else ""
            if posname in ("pre", "suf", "wp", "we", "x"):
                continue
            everything = ([word] if word else []) + [w for e in entries for w in derived_forms(e)]
            if usage and usage.group(1) in ("offensive-1", "offensive-2", "vulgar-1"):
                offensive.update(plain_form(e.strip("-@!")) for e in everything if e.strip("-@!"))
            if not variant_us(variant):
                continue
            candidates = [word] if word and word[0] not in "-@!" else []
            for e in entries:
                candidates.extend(derived_forms(e))
            for c in candidates:
                c = c.replace("’", "'")
                if c.endswith(".") and posname == "abbr":
                    c = c[:-1]
                if not c or not re.fullmatch(r"[^\W\d_]+(?:['\-][^\W\d_]+)*", c):
                    continue
                if size > SCOWL_SIZE or bumped.get(c, 0) > SCOWL_SIZE:
                    continue
                old = forms.setdefault(c, [size, 0, 0])
                old[0] = min(old[0], size)
                old[1 if posclass in NAME_CLASSES else 2] += 1
    offensive.discard("")
    # a name: every line of the spelling is a first name, surname, place or other name (French is also a language)
    return {c: [v[0], v[1] > 0 and v[2] == 0] for c, v in forms.items()}, offensive


def british_only(path: Path):
    """Spellings the variants file gives only to British, Canadian or Australian English."""
    us, other = set(), set()
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split(": ")
        if len(parts) < 2 or line[0] == "#":
            continue
        head = parts[0].replace("+ ", "")
        if not head.split() or not all(VARIANT_TOKEN.match(t) for t in head.split()):
            continue
        word = re.sub(r" ?[<{(].*$", "", parts[1]).strip()
        (us if variant_us(head) else other).add(word)
    return other - us


# --- hand lists ----------------------------------------------------------------------------------

def latin_words():
    """mimo-latin: key -> (spelling, count); hand list: key -> spelling."""
    spellings = {}
    for line in (ROOT / "DataSources/mimo-latin/latin.tsv").read_text(encoding="utf-8").splitlines():
        if line and line[0] != "#":
            key, spelling, count = line.split("\t")
            spellings[key] = (spelling, int(count))
    hand = {}
    for line in (ROOT / "Resources/Lexicon/latin.tsv").read_text(encoding="utf-8").splitlines():
        if line and line[0] != "#":
            hand[make_key(line)] = line
    return spellings, hand


def blocked_list():
    """scripts/blocked_terms.py, loaded by path (this script runs with python3 -I)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("blocked_terms", ROOT / "scripts/blocked_terms.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def blocked_keys(scowl_offensive):
    mine = {plain_form(word) for word in blocked_list().sections()["en"]}
    return mine, scowl_offensive - mine


# --- text ----------------------------------------------------------------------------------------

def tatoeba_sentences(path: Path, excluded: set):
    with bz2.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            if len(p) >= 3 and p[1] == "eng" and p[0] not in excluded:
                yield p[2]


def plain_lines(path: Path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                yield line


def chunks(text: str):
    """Sentences of a line as lists of word tokens (digits and underscores kept so that they break the chain)."""
    text = text.replace("’", "'")
    pos = 0
    for m in SENTENCE_END.finditer(text):
        if m.group(1):      # Mr. Smith: not the end
            continue
        yield WORD.findall(text[pos:m.start()])
        pos = m.end()
    yield WORD.findall(text[pos:])


def corpora(excluded: set, text_dirs):
    """(source name, lines function)."""
    yield "tatoeba", lambda: tatoeba_sentences(dl("tatoeba"), excluded)
    yield "commonvoice-en", lambda: plain_lines(dl("commonvoice-en"))
    for d in text_dirs:
        for p in sorted(Path(d).glob("*.txt")):
            yield f"text:{p.name}", (lambda p=p: plain_lines(p))


class Resolver:
    def __init__(self, forms: set, mid: Counter, blocked: set):
        self.forms = forms
        self.mid = mid
        self.blocked = blocked
        self.by_lower = {}
        for form in sorted(forms):
            self.by_lower.setdefault(deaccent(form).lower(), []).append(form)
        self.cache = {}

    def sentence_blocked(self, tokens) -> bool:
        for t in tokens:
            if plain_form(t) in self.blocked or any(plain_form(p) in self.blocked for p in t.split("-")):
                return True
        return False

    def pick(self, token: str, at_start: bool):
        forms = self.forms
        if token in forms:
            if at_start and token[0].isupper():
                lower = token.lower()
                if lower in forms and self.mid[lower] >= self.mid[token]:
                    return lower
            return token
        cands = self.by_lower.get(deaccent(token).lower())
        if cands:
            return max(cands, key=lambda c: (self.mid[c] + self.mid[c.lower()], c.islower(), c))
        return None

    def resolve(self, token: str, at_start: bool):
        """Row spelling(s) of a token, or None when it is not a word of the list. Possessives count as the noun."""
        k = (token, at_start)
        if k in self.cache:
            return self.cache[k]
        r = None
        if not re.search(r"[\d_]", token):
            r = self.pick(token, at_start)
            if r is None and token.endswith("'s"):
                r = self.pick(token[:-2], at_start)
            if r is None and "-" in token:
                r = [self.pick(p, at_start and i == 0) for i, p in enumerate(token.split("-"))]
                if None in r:
                    r = None
        self.cache[k] = r
        return r


def count(excluded_ids: set, text_dirs):
    spellings, hand = latin_words()
    forms, scowl_offensive = scowl_forms(dl("scowl-pre"), dl("scowl-exclude"))
    scowl_total = len(forms)
    gone = british_only(dl("scowl-variants"))
    removed = [w for w in forms if w in gone]
    for w in removed:
        del forms[w]
    # The words of mimo-latin and of the hand list (the hand spelling wins). A spelling that SCOWL also has in
    # lower case (App -> app) is carried by that lower-case row; the others are rows of their own.
    latin_keys = set(spellings) | set(hand)
    carriers = {}   # row spelling -> count it has in mimo-latin
    dropped_latin = []
    protected = set()
    for key in sorted(latin_keys):
        spelling = hand.get(key) or spellings[key][0]
        if not re.fullmatch(r"[A-Za-z]+(?:-[A-Za-z]+)*", spelling) or not 1 <= len(key) <= 24:
            continue
        n = spellings[key][1] if key in spellings else HAND_COUNT
        if spelling not in forms and key not in hand and spelling.lower() in forms:
            spelling = spelling.lower()
        elif spelling not in forms:
            if key not in hand and n < LATIN_ONLY_MIN:
                dropped_latin.append(spelling)
                continue
            forms[spelling] = [100, key not in hand and casing(spelling) == {"proper"}]
            protected.add(spelling)
        elif key in hand:
            protected.add(spelling)
        carriers[spelling] = max(carriers.get(spelling, 0), n)
    blocked_own, blocked_scowl = blocked_keys(scowl_offensive)
    blocked = blocked_own | blocked_scowl
    for fixed in FIXED:
        forms[fixed] = [35, False]
    for w in [w for w in forms if len(w) == 1 and w not in ("a", "I")]:
        del forms[w]

    # pass 1: how often each spelling is written in the middle of a sentence (casing evidence)
    mid = Counter()
    probe = Resolver(set(), Counter(), blocked)
    for name, lines in corpora(excluded_ids, text_dirs):
        for line in lines():
            for tokens in chunks(line):
                if not tokens or probe.sentence_blocked(tokens):
                    continue
                mid.update(tokens[1:])
                if not tokens[0][0].isupper():
                    mid[tokens[0]] += 1
    # a spelling the text writes mostly in lower case (OK, Internet): show it in lower case
    renamed = {}
    for form in sorted(forms):
        if form in protected or form in FIXED or form.islower() or form.lower() in forms:
            continue
        lo, up = mid[form.lower()], mid[form]
        if lo >= 5 and lo * LOWER_SHARE >= lo + up:
            renamed[form] = form.lower()
    for old, new in renamed.items():
        forms[new] = forms.pop(old)
        if old in carriers:
            carriers[new] = carriers.pop(old)
    resolver = Resolver(set(forms), mid, blocked)

    # pass 2: single words and pairs
    uni = {"tatoeba": Counter(), "other": Counter()}
    pairs = {"tatoeba": Counter(), "other": Counter()}
    n_sentences, skipped_blocked = Counter(), Counter()
    for name, lines in corpora(excluded_ids, text_dirs):
        group = "tatoeba" if name == "tatoeba" else "other"
        u, p = uni[group], pairs[group]
        for line in lines():
            for tokens in chunks(line):
                if not tokens:
                    continue
                if resolver.sentence_blocked(tokens):
                    skipped_blocked[name] += 1
                    continue
                n_sentences[group] += 1
                row = []
                for i, t in enumerate(tokens):
                    r = resolver.resolve(t, i == 0)
                    if isinstance(r, list):
                        row.extend(r)
                    else:
                        row.append(r)
                for w in row:
                    if w is not None:
                        u[w] += 1
                if row[0] is not None:
                    p[(START, row[0])] += 1
                if row[-1] is not None:
                    p[(row[-1], END)] += 1
                for a, b in zip(row, row[1:]):
                    if a is not None and b is not None:
                        p[(a, b)] += 1

    # first names and surnames: Tatoeba counts at most as much as the word of rank CAP_RANK
    ranked = sorted(uni["tatoeba"].values(), reverse=True)
    cap = ranked[CAP_RANK - 1]
    ratio = {w: cap / c for w, c in uni["tatoeba"].items() if forms[w][1] and c > cap}
    capped = sorted(ratio, key=lambda w: (-uni["tatoeba"][w], w))

    # rows
    rows = {}
    for w, (size, _) in forms.items():
        c = cap if w in ratio else uni["tatoeba"][w]
        weighted = (c + uni["other"][w]) * SPOKEN_WEIGHT
        if w in carriers:
            weighted = max(weighted, carriers[w])
        if plain_form(w) in blocked:      # still typed in full: a row, never suggested
            weighted = max(weighted, 1)
        if weighted > 0 and (weighted >= MIN_COUNT or size <= SMALL_SIZE or w in carriers):
            rows[w] = (make_key(w), weighted, size)
    # a second spelling of a key is a row of its own only when it is common enough; its count joins the first
    by_key = {}
    for w, (key, weighted, size) in rows.items():
        by_key.setdefault(key, []).append((weighted, w))
    merged = set()
    for key, items in by_key.items():
        items.sort(key=lambda x: (-x[0], x[1]))
        extra = 0
        for weighted, w in items[1:]:
            if weighted * FORM_SHARE < items[0][0] and w not in protected and plain_form(w) not in blocked:
                extra += weighted
                merged.add(w)
        if extra:
            first = items[0][1]
            rows[first] = (key, rows[first][1] + extra, rows[first][2])
    for w in merged:
        del rows[w]
    order = sorted(rows, key=lambda w: (-rows[w][1], rows[w][2], w))
    below_limit = order[LIMIT:]
    order = order[:LIMIT]
    kept = set(order)

    flags = {}
    for w in order:
        fl = casing(w)
        if w in carriers:
            fl.add("in-chinese")
        if plain_form(w) in blocked:
            fl.add("nosuggest")
        flags[w] = ",".join(sorted(fl)) or "-"

    # pairs: scale the names, rank by what they tell, cut
    sentences_weighted = sum(n_sentences.values()) * SPOKEN_WEIGHT
    totals = {w: rows[w][1] for w in order}
    totals[START] = totals[END] = sentences_weighted
    scaled = Counter()
    for group, p in pairs.items():
        for (a, b), c in p.items():
            if (a in kept or a == START) and (b in kept or b == END):
                if group == "tatoeba":
                    c = c * ratio.get(a, 1.0) * ratio.get(b, 1.0)
                scaled[(a, b)] += c
    N = sum(totals[w] for w in order) + sentences_weighted
    candidates = []
    for (a, b), c in scaled.items():
        w = round(c * SPOKEN_WEIGHT)
        if w < MIN_PAIR * SPOKEN_WEIGHT:
            continue
        candidates.append((w * math.log(w * N / (totals[a] * totals[b])), a, b, w))
    n_candidates = len(candidates)
    candidates.sort(key=lambda t: (-t[0], t[1], t[2]))
    cut = candidates[:PAIR_TARGET]
    cut.sort(key=lambda t: (t[1], -t[3], t[2]))

    DATA.mkdir(parents=True, exist_ok=True)
    with lzma.open(DATA / "english.tsv.xz", "wt", encoding="utf-8", preset=9) as f:
        f.write("# key\tspelling\tweighted count\tflags (scripts/mimo_english.py; see SOURCE.json)\n")
        for w in order:
            f.write(f"{rows[w][0]}\t{w}\t{rows[w][1]}\t{flags[w]}\n")
    with lzma.open(DATA / "pairs.tsv.xz", "wt", encoding="utf-8", preset=9) as f:
        f.write("# left\tright\tweighted count (spellings of english.tsv; <s> sentence start, </s> end)\n")
        for _, a, b, w in cut:
            f.write(f"{a}\t{b}\t{w}\n")

    rec = record()
    rec["counts"] = {
        "rows": len(order), "limit": LIMIT, "min_weighted_count": MIN_COUNT, "scowl_size": SCOWL_SIZE,
        "scowl_us_forms": scowl_total, "scowl_british_only_removed": len(removed),
        "rows_below_limit_dropped": len(below_limit), "mimo_latin_words_below_min_dropped": len(dropped_latin), "spellings_shown_in_lower_case": len(renamed),
        "sentences_counted": dict(sorted(n_sentences.items())),
        "sentences_with_blocked_word_left_out": dict(sorted(skipped_blocked.items())),
        "blocked_words": {"project_list": len(blocked_own), "added_from_scowl_usage_notes": len(blocked_scowl)},
        "blocked_rows_kept_as_typed_only": sum(1 for w in order if plain_form(w) in blocked),
        "name_cap": {"tatoeba_count_of_rank": CAP_RANK, "cap": cap, "names_capped": len(capped), "top": capped[:10]},
        "pairs": {"min_uses": MIN_PAIR, "candidates": n_candidates, "target": PAIR_TARGET, "kept": len(cut)},
        "spoken_weight": SPOKEN_WEIGHT,
    }
    rec["files"] = {"english.tsv.xz": sha256(DATA / "english.tsv.xz"), "pairs.tsv.xz": sha256(DATA / "pairs.tsv.xz")}
    (DATA / "SOURCE.json").write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"english.tsv.xz: {len(order)} words; pairs.tsv.xz: {len(cut)} pairs (of {n_candidates} seen {MIN_PAIR}+ times)")


def relist():
    """The [en] list applied to english.tsv.xz and pairs.tsv.xz as they are (see the module docstring)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("mimo_en_binary", ROOT / "scripts/mimo_en_binary.py")
    binary = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(binary)
    heads = {}
    for name in ("english.tsv.xz", "pairs.tsv.xz"):
        with lzma.open(DATA / name, "rt", encoding="utf-8") as f:
            heads[name] = f.readline()
    rows, pairs, marked, dropped = binary.never_volunteer(binary.read_tsv(DATA / "english.tsv.xz"), binary.read_tsv(DATA / "pairs.tsv.xz"))
    with lzma.open(DATA / "english.tsv.xz", "wt", encoding="utf-8", preset=9) as f:
        f.write(heads["english.tsv.xz"])
        for fields in rows:
            f.write("\t".join(fields) + "\n")
    with lzma.open(DATA / "pairs.tsv.xz", "wt", encoding="utf-8", preset=9) as f:
        f.write(heads["pairs.tsv.xz"])
        for fields in pairs:
            f.write("\t".join(fields) + "\n")
    rec = record()
    rec["counts"]["blocked_rows_kept_as_typed_only"] = sum(1 for fields in rows if "nosuggest" in fields[3].split(","))
    rec["counts"]["pairs"]["kept"] = len(pairs)
    rec["counts"]["relisted"] = {"words_on_the_list": marked, "pairs_left_out": dropped}
    rec["files"] = {"english.tsv.xz": sha256(DATA / "english.tsv.xz"), "pairs.tsv.xz": sha256(DATA / "pairs.tsv.xz")}
    (DATA / "SOURCE.json").write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"relist: {marked} words on the list, {dropped} pairs left out")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    f = sub.add_parser("fetch")
    f.add_argument("--seed", action="append", default=[])
    c = sub.add_parser("count")
    c.add_argument("--exclude-ids")
    c.add_argument("--text", action="append", default=[])
    sub.add_parser("relist")
    args = ap.parse_args()
    if args.cmd == "fetch":
        fetch(args.seed)
    elif args.cmd == "relist":
        relist()
    else:
        ids = set(Path(args.exclude_ids).read_text().split()) if args.exclude_ids else set()
        count(ids, args.text)


if __name__ == "__main__":
    main()
