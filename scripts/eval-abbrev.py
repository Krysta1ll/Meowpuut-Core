#!/usr/bin/env python3
"""Abbreviated input (簡拼): is the word or sentence found when only initials are typed?

  python3 scripts/eval-abbrev.py [--lib PATH] [--resources DIR] [--no-pairs]
                                 [--show-failures N] [--dump OUT.tsv] [--compare OLD.tsv]
  python3 scripts/eval-abbrev.py --make-cases     rewrite Tests/eval/abbrev-cases.tsv

The cases are read from Tests/eval/abbrev-cases.tsv, so that runs with another
lexicon are compared on the same cases. --make-cases builds them (fixed seed) from two sources:
  words      common words of 2 to 4 characters (pinyin weight 6000 or more,
             every syllable spelled) in .build/lexicon/native-pinyin.tsv
  sentences  the held-out sentences of 5 to 7 characters in
             Tests/eval/heldout-sentences.tsv
in four categories of 100 cases each (80 words, 20 sentences):
  pinyin-initials  every syllable as its initial (wmd for 我們的); zh, ch and sh
                   are typed in full in every other case, else as z, c, s
  pinyin-mixed     the first syllable in full, the rest as initials (wod, woxhn)
  zhuyin-initials  every syllable as its first symbol (ㄋㄏ for 你好, ㄨㄉ for 我的)
  zhuyin-mixed     the first syllable in full, with its tone mark in every
                   other case, the rest as first symbols (ㄨㄛˇㄒㄏ)
Pinyin cases alternate between Traditional and Simplified sessions, zhuyin
cases are Traditional. Sessions are set up the way the clients do it: the pair
table (mimo_pairs.bin) attached unless --no-pairs, and for pinyin the mixed
Japanese pool registered (as in scripts/eval-ranking.py).

Metrics per category and source: top1 and top5 (the expected text is
candidate 1, or among candidates 1 to 5, and reads the whole input), and
none (the input had no candidate at all).

--dump writes per-case results as TSV; --compare OLD.tsv lists the cases whose
top1 changed against such a dump. Without --lib the release library
target/release/libmeowpuut_ffi.* is used (it is never built here).
"""
import argparse
import ctypes
import importlib.util
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASES = ROOT / "Tests/eval/abbrev-cases.tsv"
TOP = 5
PER_CATEGORY = 100
SENTENCES_PER_CATEGORY = 20
MIN_WEIGHT = 6000
SEED = 20261114
CATEGORIES = ("pinyin-initials", "pinyin-mixed", "zhuyin-initials", "zhuyin-mixed")


def script_module(name, file):
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(ROOT / "scripts"))
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RANKING = script_module("eval_ranking", "eval-ranking.py")
GOLDEN = RANKING.golden_module()
COMPILER = script_module("compile_native_lexicon", "compile-native-lexicon.py")
SYLLABLE = __import__("re").compile(r"[a-z]+[1-5]")
TONES = " 6347"


def zhuyin_keys(syllable):
    """Da-Chien keys of a numbered pinyin syllable (wo3 -> ji3), or None."""
    return COMPILER.encode(syllable.replace("v", "ü"))


def symbols(keys):
    return "".join(GOLDEN.SYMBOL_FOR_KEY[key] for key in keys)


def pinyin_initial(syllable, long):
    """First letter of a pinyin syllable; zh, ch, sh in full when `long`."""
    return syllable[:2] if long and syllable[:2] in ("zh", "ch", "sh") else syllable[0]


def read_words(resources):
    """(hant, hans, [numbered syllables]) of the common words, heaviest reading of each text."""
    best = {}
    for bare, reading, hant, hans, weight in GOLDEN.lexicon_rows(resources / "native-pinyin.tsv"):
        weight = int(weight)
        syllables = SYLLABLE.findall(reading)
        if weight < MIN_WEIGHT or not 2 <= len(hant) <= 4 or "".join(syllables) != reading or len(syllables) != len(hant):
            continue
        if not all("一" <= c <= "鿿" for c in hant + hans):
            continue
        if not all(zhuyin_keys(s) for s in syllables):
            continue
        if weight > best.get(hant, (0,))[0]:
            best[hant] = (weight, hans, syllables)
    return sorted((hant, hans, syllables) for hant, (_, hans, syllables) in best.items())


def read_sentences(path):
    """(script, sentence, [toneless syllables]) of the short held-out sentences."""
    rows = []
    for script, sentence, pinyin in RANKING.read_sentences(path):
        syllables = pinyin.split()
        if 5 <= len(sentence) <= 7 and len(syllables) == len(sentence) and all(zhuyin_keys(s + "1") for s in syllables):
            rows.append((script, sentence, syllables))
    return rows


def make_cases(words, sentences):
    """[(category, source, script, keys typed, expected text)], 100 per category."""
    rng = random.Random(SEED)
    cases = []
    for number, category in enumerate(CATEGORIES):
        chosen_words = rng.sample(words, PER_CATEGORY - SENTENCES_PER_CATEGORY)
        # Zhuyin is typed with Traditional output: Traditional sentences only.
        pool = [row for row in sentences if category.startswith("pinyin") or row[0] == "t"]
        chosen_sentences = rng.sample(pool, min(SENTENCES_PER_CATEGORY, len(pool)))
        items = [("word", hant, hans, syllables, True) for hant, hans, syllables in chosen_words]
        for script, sentence, syllables in chosen_sentences:
            # Held-out pinyin has no tones; a first tone stands in for zhuyin keys, and no mark is typed.
            items.append(("sentence", sentence if script == "t" else None, sentence if script == "s" else None,
                          [s + "1" for s in syllables], False))
        for index, (source, hant, hans, syllables, toned) in enumerate(items):
            if category.startswith("pinyin"):
                script = "s" if (hant is None or (index % 2 and hans is not None)) else "t"
                expected = hans if script == "s" else hant
                plain = [s[:-1] for s in syllables]
                if category == "pinyin-initials":
                    keys = "".join(pinyin_initial(s, index % 2 == 0) for s in plain)
                else:
                    keys = plain[0] + "".join(pinyin_initial(s, index % 2 == 0) for s in plain[1:])
            else:
                script, expected = "t", hant
                encoded = [zhuyin_keys(s) for s in syllables]
                if category == "zhuyin-initials":
                    keys = "".join(e[0] for e in encoded)
                else:
                    first = encoded[0] if toned and index % 2 == 0 else encoded[0][:-1]
                    keys = first + "".join(e[0] for e in encoded[1:])
                keys = symbols(keys)
            cases.append((category, source, script, keys, expected))
    return cases


class Engine(RANKING.Engine):
    """Pinyin and zhuyin lexicons with the pair table, sessions kept per mode and script."""

    def __init__(self, core, resources, pairs):
        super().__init__(core, resources, pairs)
        self.zhuyin = core.mpn_lexicon_create_from_base(self.base, len(self.base), 0)
        if not self.zhuyin:
            sys.exit("eval-abbrev: mimo_base.bin did not load (zhuyin)")
        if pairs and self.pairs == "on":
            self.zhuyin_pairs = ctypes.create_string_buffer((resources / "mimo_pairs.bin").read_bytes())
            if not core.mpn_lexicon_attach_pairs(self.zhuyin, self.zhuyin_pairs, len(self.zhuyin_pairs) - 1, None, None):
                sys.exit("eval-abbrev: mimo_pairs.bin rejected by the zhuyin lexicon")
        self.sessions = {}

    def candidates(self, pinyin, script, keys):
        core = self.core
        key = (pinyin, script)
        if key not in self.sessions:
            if pinyin:
                self.sessions[key] = self.session(script)
            else:
                session = core.mpn_session_create(self.zhuyin)
                core.mpn_set_simplified(session, 1 if script == "s" else 0)
                self.sessions[key] = session
        session = self.sessions[key]
        core.mpn_clear(session)
        for symbol in keys:
            core.mpn_key(session, ord(symbol if pinyin else GOLDEN.KEY_FOR_SYMBOL[symbol]))
        typed = len(core.mpn_input(session) or b"")
        count = core.mpn_candidate_count(session)
        found = []
        for index in range(min(count, TOP)):
            text = core.mpn_candidate(session, index).decode("utf-8")
            whole = core.mpn_candidate_consumed(session, index) == typed
            found.append(text if whole else text + "…")
        return count, found


def evaluate(engine, cases):
    results = []
    for category, source, script, keys, expected in cases:
        count, found = engine.candidates(category.startswith("pinyin"), script, keys)
        rank = found.index(expected) + 1 if expected in found else 0
        results.append({"category": category, "source": source, "script": script, "keys": keys,
                        "expected": expected, "rank": rank, "count": count, "found": " ".join(found)})
    return results


def summarise(results):
    lines = [f"{'category':<18}{'source':<10}{'n':>5}{'top1':>8}{'top5':>8}{'none':>6}"]
    for category in CATEGORIES:
        rows = [r for r in results if r["category"] == category]
        for source in ("word", "sentence", "all"):
            group = [r for r in rows if source == "all" or r["source"] == source]
            if not group:
                continue
            n = len(group)
            lines.append(f"{category:<18}{source:<10}{n:>5}"
                         f"{100 * sum(r['rank'] == 1 for r in group) / n:>7.1f}%"
                         f"{100 * sum(r['rank'] >= 1 for r in group) / n:>7.1f}%"
                         f"{sum(r['count'] == 0 for r in group):>6}")
    n = len(results)
    lines.append(f"{'TOTAL':<28}{n:>5}{100 * sum(r['rank'] == 1 for r in results) / n:>7.1f}%"
                 f"{100 * sum(r['rank'] >= 1 for r in results) / n:>7.1f}%{sum(r['count'] == 0 for r in results):>6}")
    lines.append("top1/top5 = the expected text is candidate 1 / among 1-5 and reads the whole input; "
                 "none = no candidate at all.")
    return "\n".join(lines)


DUMP_FIELDS = ("category", "source", "script", "keys", "expected", "rank", "count", "found")


def dump(results, path):
    with open(path, "w", encoding="utf-8") as out:
        out.write("\t".join(DUMP_FIELDS) + "\n")
        for r in results:
            out.write("\t".join(str(r[field]) for field in DUMP_FIELDS) + "\n")


def compare(results, path):
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    fields = lines[0].split("\t")
    old = {}
    for line in lines[1:]:
        row = dict(zip(fields, line.split("\t")))
        old[(row["category"], row["script"], row["keys"], row["expected"])] = row
    better, worse = [], []
    for r in results:
        before = old.get((r["category"], r["script"], r["keys"], r["expected"]))
        if before is None:
            continue
        was, now = int(before["rank"]), r["rank"]
        if (was == 1) != (now == 1):
            (better if now == 1 else worse).append((r, before))
    print(f"\ncompare with {path}: top1 +{len(better)} -{len(worse)}")
    for title, items in (("now top1", better), ("no longer top1", worse)):
        if items:
            print(f"  {title}:")
        for r, before in items:
            print(f"    {r['category']} [{r['script']}] {r['keys']} -> {r['expected']} | was {before['found']} | now {r['found']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sentences", default=str(ROOT / "Tests/eval/heldout-sentences.tsv"))
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--lib", default=None)
    parser.add_argument("--no-pairs", action="store_true", help="do not attach mimo_pairs.bin")
    parser.add_argument("--show-failures", type=int, default=0, metavar="N")
    parser.add_argument("--dump", metavar="PATH")
    parser.add_argument("--compare", metavar="OLD")
    parser.add_argument("--make-cases", action="store_true", help=f"rewrite {CASES.relative_to(ROOT)}")
    arguments = parser.parse_args()
    lib = Path(arguments.lib) if arguments.lib else RANKING.default_lib()
    if not lib.exists():
        sys.exit(f"eval-abbrev: {lib} not found (build it with cargo build --release -p meowpuut-ffi)")
    resources = Path(arguments.resources)
    if arguments.make_cases:
        cases = make_cases(read_words(resources), read_sentences(arguments.sentences))
        head = ("# Cases of scripts/eval-abbrev.py, written by --make-cases: "
                "category, source, script, keys typed, expected text.\n")
        CASES.write_text(head + "".join("\t".join(case) + "\n" for case in cases), encoding="utf-8")
        print(f"wrote {CASES}: {len(cases)} cases")
        return 0
    cases = [tuple(line.split("\t")) for line in CASES.read_text(encoding="utf-8").splitlines()
             if line and not line.startswith("#")]
    engine = Engine(RANKING.load_core(lib), resources, not arguments.no_pairs)
    results = evaluate(engine, cases)
    print(f"{len(cases)} cases; pairs {engine.pairs}")
    print(summarise(results))
    if arguments.show_failures:
        failures = [r for r in results if r["rank"] != 1]
        print(f"\nnot top1 ({min(len(failures), arguments.show_failures)} of {len(failures)}): keys -> expected | top {TOP}")
        for r in failures[:arguments.show_failures]:
            print(f"  {r['category']} [{r['script']}] {r['keys']} -> {r['expected']} | {r['found']}")
    if arguments.dump:
        dump(results, arguments.dump)
        print(f"\nwrote {arguments.dump}")
    if arguments.compare:
        compare(results, arguments.compare)
    return 0


if __name__ == "__main__":
    sys.exit(main())
