#!/usr/bin/env python3
"""Measure nine-key (telephone keypad) Pinyin input.

  python3 scripts/eval-nine-key.py [--lib PATH] [--resources DIR] [--items-from DIR] [--show-failures N]

The word samples are drawn from the lexicon under test; --items-from draws them from another build's
native-pinyin.tsv instead, so two builds can be compared on the same cases.

Test items (deterministic, seeded), typed into a fresh Simplified nine-key session:
  * initials  ~120 common words (2-4 Han characters, weight >= 6000, the heaviest
              entry of their bare pinyin), each syllable typed as the key of its
              first letter (现在: 99).
  * mixed     ~40 of those words with the first syllable spelled and the rest as
              initials (现在: 9426 9).
  * spelled   ~100 of those words spelled in full (现在: 9426924); the guard,
              it must not get worse.
  * typing    ~300 states of the Simplified held-out sentences
              (Tests/eval/heldout-sentences.tsv) while they are typed: some
              syllables spelled and the first key of the next (现在是: 9426 9).
              A candidate counts when it is the start of the sentence and reads
              all the spelled syllables (现, 现在, not 先 or 现象); the guard for
              input in the middle of a word, it must not get worse.
  * english   ~50 words of the Latin list (OK, iPhone, Wi-Fi), the keys of their
              letters (OK: 65).
The first 10 candidates are read and the rank of the expected text recorded.
BENCH prints the time of one keystroke as seen from Python (ctypes overhead included).
"""
import argparse
import ctypes
import importlib.util
import random
import statistics
import sys
import time
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = 20261114
TOP = 10
WORD_WEIGHT = 6000
COUNTS = {"initials": 120, "mixed": 40, "spelled": 100, "typing": 300, "english": 50}
KEYS = {letter: str(digit) for digit, letters in
        ((2, "abc"), (3, "def"), (4, "ghi"), (5, "jkl"), (6, "mno"), (7, "pqrs"), (8, "tuv"), (9, "wxyz"))
        for letter in letters}


def load_golden_module():
    spec = importlib.util.spec_from_file_location("check_chinese_golden", ROOT / "scripts/check-chinese-golden.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GOLDEN = load_golden_module()


def is_han(text):
    return all("㐀" <= ch <= "鿿" or "豈" <= ch <= "﫿" for ch in text)


def digits(letters):
    return "".join(KEYS[ch] for ch in letters.lower())


def syllables(toned):
    return "".join(ch if not ch.isdigit() else " " for ch in toned).split()


def build_items(path):
    heaviest = {}
    latin = []
    for fields in GOLDEN.lexicon_rows(path):
        bare, toned, _, hans, weight = fields[0], fields[1], fields[2], fields[3], int(fields[4])
        if bare.isascii() and bare.isalpha() and bare.isupper():
            latin.append((bare, hans, weight))
        elif bare.isascii() and bare.isalpha() and bare.islower():
            if bare not in heaviest or weight > heaviest[bare][2]:
                heaviest[bare] = (toned, hans, weight)
    words = sorted((toned, hans) for bare, (toned, hans, weight) in heaviest.items()
                   if weight >= WORD_WEIGHT and 2 <= len(hans) <= 4 and is_han(hans)
                   and len(syllables(toned)) == len(hans) and "v" not in toned and "ue" not in bare)
    rng = random.Random(SEED)
    items = []
    for toned, hans in sorted(rng.sample(words, min(COUNTS["initials"], len(words)))):
        items.append(("initials", digits("".join(s[0] for s in syllables(toned))), hans))
    for toned, hans in sorted(rng.sample(words, min(COUNTS["mixed"], len(words)))):
        parts = syllables(toned)
        items.append(("mixed", digits(parts[0] + "".join(s[0] for s in parts[1:])), hans))
    for toned, hans in sorted(rng.sample(words, min(COUNTS["spelled"], len(words)))):
        items.append(("spelled", digits("".join(syllables(toned))), hans))
    # Its own generator, so adding this kind left the other samples as they were.
    states = []
    for line in (ROOT / "Tests/eval/heldout-sentences.tsv").read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.startswith("s\t"):
            continue
        _, sentence, pinyin = line.split("\t")
        parts = pinyin.split()
        if len(parts) == len(sentence) and all(p.isascii() and p.isalpha() for p in parts):
            states.extend((digits("".join(parts[:i]) + parts[i][0]), sentence + "\t" + sentence[:i])
                          for i in range(1, len(parts)))
    for keys, expected in sorted(random.Random(SEED + 1).sample(states, min(COUNTS["typing"], len(states)))):
        items.append(("typing", keys, expected))
    english = sorted((bare, text) for bare, text, weight in latin if 2 <= len(bare) <= 12)
    for bare, text in sorted(rng.sample(english, min(COUNTS["english"], len(english)))):
        items.append(("english", digits(bare), text))
    for keys, text in (("65", "OK"), ("474663", "iPhone"), ("277", "App"), ("9434", "Wi-Fi")):
        items.append(("english", keys, text))
    return items


class Engine:
    def __init__(self, lib, resources):
        self.core = GOLDEN.load_core(lib)
        self.core.mpn_nine_key_session_create.restype = ctypes.c_void_p
        self.core.mpn_nine_key_session_create.argtypes = (ctypes.c_void_p,)
        base = (resources / "mimo_base.bin").read_bytes()
        self.lexicon = self.core.mpn_lexicon_create_from_base(base, len(base), 1)
        if not self.lexicon:
            sys.exit("eval-nine-key: mimo_base.bin did not load")
        self.session = self.core.mpn_nine_key_session_create(self.lexicon)
        self.core.mpn_set_simplified(self.session, 1)
        self.times = []

    def top(self, keys):
        core, session = self.core, self.session
        core.mpn_clear(session)
        for ch in keys:
            start = time.perf_counter()
            core.mpn_key(session, ord(ch))
            self.times.append(time.perf_counter() - start)
        count = core.mpn_candidate_count(session)
        result = [core.mpn_candidate(session, i).decode("utf-8") for i in range(min(count, TOP))]
        core.mpn_clear(session)
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--lib", default=str(ROOT / "target/release/libmeowpuut_ffi.dylib"))
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--items-from", help="draw the word samples from this build's native-pinyin.tsv")
    parser.add_argument("--show-failures", type=int, default=0)
    args = parser.parse_args()
    resources = Path(args.resources)
    items = build_items(Path(args.items_from or resources) / "native-pinyin.tsv")
    engine = Engine(args.lib, resources)
    stats = defaultdict(lambda: [0, 0, 0, 0, 0])  # cases, top1, top3, top7, top10
    failures = defaultdict(list)
    for kind, keys, expected in items:
        got = engine.top(keys)
        if kind == "typing":
            sentence, prefix = expected.split("\t")
            rank = next((i + 1 for i, text in enumerate(got)
                         if text.startswith(prefix) and sentence.startswith(text)), 0)
        else:
            rank = got.index(expected) + 1 if expected in got else 0
        s = stats[kind]
        s[0] += 1
        s[1] += rank == 1
        s[2] += 1 <= rank <= 3
        s[3] += 1 <= rank <= 7
        s[4] += 1 <= rank <= 10
        if rank != 1:
            failures[kind].append(f"{kind} {keys} expected={expected} rank={rank or '-'} got={got[:5]}")
    print(f"items: {len(items)}; seed {SEED}; Simplified session")
    print(f"{'kind':<12}{'cases':>7}{'top1%':>8}{'top3%':>8}{'top7%':>8}{'top10%':>8}")
    for kind in COUNTS:
        s = stats[kind]
        cases = s[0] or 1
        print(f"{kind:<12}{s[0]:>7}" + "".join(f"{100 * v / cases:>8.1f}" for v in s[1:]))
    ms = sorted(t * 1000 for t in engine.times)
    print(f"BENCH nine-key keystroke: median {statistics.median(ms):.3f} ms; "
          f"p99 {ms[int(len(ms) * 0.99)]:.3f} ms; max {ms[-1]:.3f} ms ({len(ms)} keys)")
    if args.show_failures:
        print("\nfailures (not rank 1):")
        for kind in COUNTS:
            for text in failures[kind][:args.show_failures]:
                print("  " + text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
