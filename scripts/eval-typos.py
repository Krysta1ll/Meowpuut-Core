#!/usr/bin/env python3
"""Measure how well the Pinyin decoder recovers from typing mistakes.

  python3 scripts/eval-typos.py [--lib PATH] [--resources DIR] [--items-from DIR] [--limit N] [--show-failures N]

--items-from draws the test items from another build's native-pinyin.tsv (the same cases for two builds).

Test items
  * ~600 common words from native-pinyin.tsv (2-4 Han characters, weight >= 6000,
    the heaviest entry of their bare pinyin), each in a Traditional and a
    Simplified session. Expected text = the word in that script.
  * 150 whole phrases from Tests/golden/chinese-pinyin-inputs.txt (8+ letters).
    Expected text = the current top candidate of the correct input.

Every item gets one case per category (deterministic, seeded):
  neighbor, swap, omit, double, extra, fuzzy_initial, fuzzy_final, clean.
A mutation is skipped when it spells the bare pinyin of another word with
weight >= 3000 (that is a real word, not a typo), or when it does not apply.

Each case is typed into a fresh composition; the first 8 candidates are read
and the rank of the expected text recorded. "clean" is the regression guard:
its top-1 must stay high.
"""
import argparse
import ctypes
import importlib.util
import json
import random
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = 20261114
WORDS = 600
PHRASES = 150
TOP = 8
REAL_WEIGHT = 3000
WORD_WEIGHT = 6000
CATEGORIES = ["neighbor", "swap", "omit", "double", "extra", "fuzzy_initial", "fuzzy_final", "clean"]


def load_golden_module():
    spec = importlib.util.spec_from_file_location("check_chinese_golden", ROOT / "scripts/check-chinese-golden.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GOLDEN = load_golden_module()

# QWERTY neighbours: same row left/right, plus the staggered rows above and below.
ROWS = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]


def qwerty_neighbors():
    where = {key: (r, c) for r, row in enumerate(ROWS) for c, key in enumerate(row)}
    result = {}
    for key, (r, c) in where.items():
        near = []
        for rr, cc in ((r, c - 1), (r, c + 1), (r - 1, c), (r - 1, c + 1), (r + 1, c - 1), (r + 1, c)):
            if 0 <= rr < len(ROWS) and 0 <= cc < len(ROWS[rr]):
                near.append(ROWS[rr][cc])
        result[key] = near
    return result


NEIGHBORS = qwerty_neighbors()
INITIAL_SWAPS = [("zh", "z"), ("ch", "c"), ("sh", "s"), ("n", "l"), ("f", "h"), ("r", "l")]
FINAL_SWAPS = [("iang", "ian"), ("uang", "uan"), ("ang", "an"), ("eng", "en"), ("ing", "in")]


def is_han(text):
    return all("㐀" <= ch <= "鿿" or "豈" <= ch <= "﫿" for ch in text)


def read_lexicon(path):
    rows = [fields for fields in GOLDEN.lexicon_rows(path) if fields[0].isascii() and fields[0].isalpha()
            and fields[0].islower()]
    syllables = set()
    for bare, toned, _, _, _ in rows:
        for part in "".join(ch if not ch.isdigit() else " " for ch in toned).split():
            if part.isalpha() and part.islower():
                syllables.add(part)
    return rows, syllables


def segment(keys, syllables):
    """Split into pinyin syllables (fewest syllables); None when impossible."""
    best = [None] * (len(keys) + 1)
    best[0] = []
    for end in range(1, len(keys) + 1):
        for start in range(max(0, end - 6), end):
            if best[start] is not None and keys[start:end] in syllables:
                option = best[start] + [keys[start:end]]
                if best[end] is None or len(option) < len(best[end]):
                    best[end] = option
    return best[len(keys)]


def mutate(category, keys, sylls, rng):
    """One typo of the category or None when it does not apply."""
    n = len(keys)
    if category == "clean":
        return keys
    if category == "neighbor":
        i = rng.randrange(n)
        return keys[:i] + rng.choice(NEIGHBORS[keys[i]]) + keys[i + 1:]
    if category == "swap":
        spots = [i for i in range(n - 1) if keys[i] != keys[i + 1]]
        if not spots:
            return None
        i = rng.choice(spots)
        return keys[:i] + keys[i + 1] + keys[i] + keys[i + 2:]
    if category == "omit":
        i = rng.randrange(1, n)
        return keys[:i] + keys[i + 1:]
    if category == "double":
        i = rng.randrange(n)
        return keys[:i + 1] + keys[i] + keys[i + 1:]
    if category == "extra":
        i = rng.randrange(n)
        extra = rng.choice(NEIGHBORS[keys[i]])
        return keys[:i] + extra + keys[i:] if rng.random() < 0.5 else keys[:i + 1] + extra + keys[i + 1:]
    if sylls is None:
        return None
    options = []
    for index, syl in enumerate(sylls):
        if category == "fuzzy_initial":
            for a, b in INITIAL_SWAPS:
                if syl.startswith(a) and not (len(a) == 1 and syl[:2] in ("zh", "ch", "sh")):
                    options.append((index, b + syl[len(a):]))
                elif syl.startswith(b) and not (len(b) == 1 and syl[:2] in ("zh", "ch", "sh")):
                    options.append((index, a + syl[len(b):]))
        else:
            for a, b in FINAL_SWAPS:
                if "uan" in (a, b) and syl[0] in "jqxy":
                    continue  # juan/quan/xuan/yuan are u-umlaut finals, no uang counterpart
                if syl.endswith(a):
                    options.append((index, syl[:-len(a)] + b))
                    break
                if syl.endswith(b):
                    options.append((index, syl[:-len(b)] + a))
                    break
    if not options:
        return None
    index, changed = rng.choice(options)
    return "".join(sylls[:index]) + changed + "".join(sylls[index + 1:])


class Engine:
    def __init__(self, lib, resources, pairs=False, trigrams=False):
        self.core = GOLDEN.load_core(lib)
        ptr, text, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t
        for name in ("mpn_lexicon_attach_pairs", "mpn_lexicon_attach_trigrams"):
            fn = getattr(self.core, name)
            fn.restype, fn.argtypes = ctypes.c_int, (ptr, text, size, ptr, ptr)
        self.base = (resources / "mimo_base.bin").read_bytes()
        self.lexicon = self.core.mpn_lexicon_create_from_base(self.base, len(self.base), 1)
        if not self.lexicon:
            sys.exit("eval-typos: mimo_base.bin did not load")
        self._bufs = []
        if pairs:
            p = resources / "mimo_pairs.bin"
            if p.exists():
                b = p.read_bytes()
                self._bufs.append(b)
                if not self.core.mpn_lexicon_attach_pairs(self.lexicon, b, len(b), None, None):
                    sys.exit("eval-typos: mimo_pairs.bin did not attach")
        if trigrams:
            t = resources / "mimo_trigrams.bin"
            if t.exists():
                b = t.read_bytes()
                self._bufs.append(b)
                if not self.core.mpn_lexicon_attach_trigrams(self.lexicon, b, len(b), None, None):
                    sys.exit("eval-typos: mimo_trigrams.bin did not attach")
        pool = json.loads((resources / "native-japanese.json").read_text(encoding="utf-8"))
        self.sessions = {}
        for script in "ts":
            session = self.core.mpn_session_create(self.lexicon)
            self.core.mpn_set_simplified(session, 1 if script == "s" else 0)
            for reading, entries in pool.items():
                if entries:
                    self.core.mpn_add_japanese_word(session, reading.encode(), entries[0]["text"].encode(),
                                                    entries[0]["weight"])
            self.sessions[script] = session

    def top(self, script, keys):
        core, session = self.core, self.sessions[script]
        core.mpn_clear(session)
        for ch in keys:
            core.mpn_key(session, ord(ch))
        count = core.mpn_candidate_count(session)
        result = [core.mpn_candidate(session, i).decode("utf-8") for i in range(min(count, TOP))]
        core.mpn_clear(session)
        return result


def run_real_word_benchmark(engine, syllables, show_failures):
    """Test 'fake-valid' typos on real sentences whose mutated keys still segment into valid syllables."""
    sent_files = [ROOT / "Tests/eval/heldout-sentences.tsv", ROOT / "Tests/eval/modern-stress-sentences.tsv"]
    rows = []
    for path in sent_files:
        if not path.exists():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 3 or parts[0] not in ("t", "s"):
                continue
            script, expected, py = parts[0], parts[1].strip(), "".join(parts[2].split())
            if not (6 <= len(py) <= 30 and py.isascii() and py.isalpha() and py.islower() and is_han(expected)):
                continue
            rows.append((script, expected, py))
    stats = defaultdict(lambda: [0, 0, 0, 0])  # cases, top1, top5, top8
    failures = defaultdict(list)
    for idx, (script, expected, py) in enumerate(rows):
        clean_top = engine.top(script, py)
        if not clean_top or clean_top[0] != expected:
            continue
        # Only test sentences the clean engine gets right so we isolate typo recovery.
        for cat in ("rw_omit", "rw_neighbor", "rw_swap"):
            rng = random.Random(f"{SEED}:rw:{idx}:{cat}")
            cands = []
            if cat == "rw_omit":
                for i in range(1, len(py)):
                    m = py[:i] + py[i + 1:]
                    if m != py and segment(m, syllables) is not None:
                        cands.append(m)
            elif cat == "rw_neighbor":
                for i, ch in enumerate(py):
                    for nb in NEIGHBORS[ch]:
                        m = py[:i] + nb + py[i + 1:]
                        if m != py and segment(m, syllables) is not None:
                            cands.append(m)
            else:
                for i in range(len(py) - 1):
                    if py[i] != py[i + 1]:
                        m = py[:i] + py[i + 1] + py[i] + py[i + 2:]
                        if m != py and segment(m, syllables) is not None:
                            cands.append(m)
            if not cands:
                continue
            typed = rng.choice(cands)
            got = engine.top(script, typed)
            rank = got.index(expected) + 1 if expected in got else 0
            for g in (cat, "rw_TOTAL"):
                s = stats[g]
                s[0] += 1
                s[1] += rank == 1
                s[2] += 1 <= rank <= 5
                s[3] += 1 <= rank <= 8
            if rank != 1:
                failures[cat].append(f"{cat} [{script}] {typed} (from {py}) expected={expected} got={got[:3]}")
    print("\n-- real-word ('fake-valid') sentence typos (mutated input still segments into valid syllables)")
    print(f"{'category':<28}{'cases':>7}{'top1%':>8}{'top5%':>8}{'top8%':>8}")
    for cat in ("rw_omit", "rw_neighbor", "rw_swap", "rw_TOTAL"):
        s = stats[cat]
        cases = s[0] or 1
        print(f"{cat:<28}{s[0]:>7}{100 * s[1] / cases:>8.1f}{100 * s[2] / cases:>8.1f}{100 * s[3] / cases:>8.1f}")
    if show_failures:
        pick = random.Random(SEED)
        for cat in ("rw_omit", "rw_neighbor", "rw_swap"):
            found = failures[cat]
            for text in sorted(pick.sample(found, min(show_failures, len(found))), key=found.index):
                print("  " + text)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--lib", default=str(ROOT / "target/release/libmeowpuut_ffi.dylib"))
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--limit", type=int, default=0, help="use only the first N items (0 = all)")
    parser.add_argument("--show-failures", type=int, default=0)
    parser.add_argument("--items-from", help="draw the items from this build's native-pinyin.tsv")
    parser.add_argument("--pairs", action="store_true", help="attach mimo_pairs.bin")
    parser.add_argument("--trigrams", action="store_true", help="attach mimo_trigrams.bin")
    parser.add_argument("--real-word", action="store_true", help="also evaluate fake-valid sentence typos")
    args = parser.parse_args()
    resources = Path(args.resources)

    rows, syllables = read_lexicon(Path(args.items_from or resources) / "native-pinyin.tsv")
    heaviest = {}
    real = set()
    for fields in rows:
        bare, weight = fields[0], int(fields[4])
        if weight >= REAL_WEIGHT:
            real.add(bare)
        if bare not in heaviest or weight > int(heaviest[bare][4]):
            heaviest[bare] = fields
    pool = sorted(bare for bare, f in heaviest.items()
                  if int(f[4]) >= WORD_WEIGHT and 2 <= len(f[2]) <= 4 and is_han(f[2]) and is_han(f[3]))
    rng = random.Random(SEED)
    chosen = sorted(rng.sample(pool, min(WORDS, len(pool))))

    items = []  # (kind, script, keys, expected or None)
    for bare in chosen:
        f = heaviest[bare]
        items.append(("word", "t", bare, f[2]))
        items.append(("word", "s", bare, f[3]))
    golden = [(s, k) for s, k in GOLDEN.read_inputs(ROOT / "Tests/golden/chinese-pinyin-inputs.txt")
              if len(k) >= 8 and k.isalpha() and k.islower()]
    golden = sorted(set(golden))
    for script, keys in sorted(rng.sample(golden, min(PHRASES, len(golden)))):
        items.append(("phrase", script, keys, None))
    if args.limit:
        items = items[:args.limit]

    engine = Engine(args.lib, resources, pairs=args.pairs, trigrams=args.trigrams)
    stats = defaultdict(lambda: [0, 0, 0, 0, 0])  # cases, top1, top5, top8, skipped
    failures = defaultdict(list)
    for item_index, (kind, script, keys, expected) in enumerate(items):
        if expected is None:
            first = engine.top(script, keys)
            if not first:
                continue
            expected = first[0]
        sylls = segment(keys, syllables)
        for category in CATEGORIES:
            case_rng = random.Random(f"{SEED}:{item_index}:{category}")
            typed = mutate(category, keys, sylls, case_rng)
            groups = [(category,), (category, kind), (category, script), ("TOTAL", script), ("TOTAL", kind)]
            if category == "clean":
                groups = [(category,), (category, kind), (category, script)]
            if typed is None or (category != "clean" and (typed == keys or typed in real)):
                for g in groups:
                    stats[g][4] += 1
                continue
            got = engine.top(script, typed)
            rank = got.index(expected) + 1 if expected in got else 0
            for g in groups:
                s = stats[g]
                s[0] += 1
                s[1] += rank == 1
                s[2] += 1 <= rank <= 5
                s[3] += 1 <= rank <= 8
            if rank != 1:
                failures[category].append(f"{category} [{script}] {typed} (from {keys}) "
                                          f"expected={expected} got={got[:5]}")

    def line(label, s):
        cases = s[0] or 1
        return (f"{label:<28}{s[0]:>7}{100 * s[1] / cases:>8.1f}{100 * s[2] / cases:>8.1f}"
                f"{100 * s[3] / cases:>8.1f}{s[4]:>9}")

    print(f"items: {sum(1 for i in items if i[0] == 'word')} word cases (t+s), "
          f"{sum(1 for i in items if i[0] == 'phrase')} phrases; seed {SEED}")
    print(f"{'category':<28}{'cases':>7}{'top1%':>8}{'top5%':>8}{'top8%':>8}{'skipped':>9}")
    for category in CATEGORIES:
        print(line(category, stats[(category,)]))
    print("-- by item kind")
    for category in CATEGORIES:
        for kind in ("word", "phrase"):
            print(line(f"{category}/{kind}", stats[(category, kind)]))
    print("-- per script (typo categories, clean excluded)")
    for script in "ts":
        print(line(f"TOTAL typos [{script}]", stats[("TOTAL", script)]))
        print(line(f"clean [{script}]", stats[("clean", script)]))
    for kind in ("word", "phrase"):
        print(line(f"TOTAL typos /{kind}", stats[("TOTAL", kind)]))

    if args.show_failures:
        print("\nfailures (not rank 1):")
        pick = random.Random(SEED)
        for category in CATEGORIES:
            found = failures[category]
            for text in sorted(pick.sample(found, min(args.show_failures, len(found))), key=found.index):
                print("  " + text)
    if args.real_word:
        run_real_word_benchmark(engine, syllables, args.show_failures)
    return 0


if __name__ == "__main__":
    sys.exit(main())
