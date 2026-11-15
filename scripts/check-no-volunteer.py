#!/usr/bin/env python3
"""Checks that the keyboard never volunteers a word of the never-volunteer list, and still types it.

  python3 scripts/check-no-volunteer.py [--resources DIR] [--lib PATH] [--quick] [--show N]

The words come from Resources/Lexicon/blocked-terms.txt (read with scripts/blocked_terms.py) and the
rows the build marked with it (the sixth field `n` of native-pinyin.tsv and native-zhuyin.tsv); none
is written here. Through the C interface of the release library (mimo_base.bin with mimo_pairs.bin
and mimo_en.bin attached, as the clients load them) it asks:

  predictions  mpn_predict after the 6,000 heaviest words, after each first part of every marked word
               (the first character of a three-character word) and after every marked word, Traditional and Simplified;
               a prediction may not be a marked word nor finish one with the last three characters
               before it (the text the predictor reads)
  prefixes     every key while a marked word is typed in full Pinyin and Zhuyin (each shorter input
               is a completion of it)
  initials     the initials of every marked word (Pinyin with and without zh/ch/sh, Zhuyin first
               symbols, nine-key digits) and its first syllable followed by initials
  typos        one-key typos and fuzzy sounds of every marked word's Pinyin (a key dropped, doubled
               or swapped, a neighbouring key, n/ng, z/zh)
  english      completions of every prefix, corrections of one-key typos, nine-key digits of every
               prefix of the [en] words, next words after the 2,000 heaviest English words and after
               every [en] word
  emoji        associations.json: no key with a listed term, no flag or banned symbol

A candidate volunteers a marked word when it is the word and the keys are not its full spelling, or
when it is not itself a lexicon word and holds a marked word that the keys do not spell. A
prediction volunteers when it is a marked word or completes one with the text before it.
It also checks that every marked word typed in full is among the first 20 candidates: in Pinyin, in
Zhuyin with every tone and in Zhuyin with the tone of the last syllable only (as people type every
other word; not where those keys also read as fewer syllables, ㄕㄨㄟˇ for 水), and that every [zh] term of the list is a marked word of its own (a term that is no
lexicon word is still built from smaller words, and the engine knows the list by its marked rows).
Exit 0 when nothing is volunteered and nothing is lost, 1 otherwise.
"""
import argparse
import collections
import ctypes
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

TONES_ZHUYIN = " 6347"
DIGITS = "22233344455566677778889999"
NEIGHBOURS = {
    "q": "wa", "w": "qes", "e": "wrd", "r": "etf", "t": "ryg", "y": "tuh", "u": "yij", "i": "uok", "o": "ipl", "p": "o",
    "a": "qsz", "s": "awdx", "d": "sefc", "f": "drgv", "g": "fthb", "h": "gyjn", "j": "hukm", "k": "jil", "l": "ko",
    "z": "asx", "x": "zsdc", "c": "xdfv", "v": "cfgb", "b": "vghn", "n": "bhjm", "m": "njk",
}
TOP = 10
EXACT_TOP = 20


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


def load_core(lib):
    core = ctypes.CDLL(str(lib))
    ptr, text, number, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    for name, result, arguments in (
            ("mpn_lexicon_create_from_base", ptr, (text, size, number)),
            ("mpn_lexicon_attach_pairs", number, (ptr, text, size, ptr, ptr)),
            ("mpn_lexicon_attach_english", number, (ptr, text, size, ptr, ptr)),
            ("mpn_session_create", ptr, (ptr,)),
            ("mpn_nine_key_session_create", ptr, (ptr,)),
            ("mpn_set_simplified", None, (ptr, number)),
            ("mpn_add_japanese_word", number, (ptr, text, text, number)),
            ("mpn_key", number, (ptr, ctypes.c_int32)),
            ("mpn_clear", None, (ptr,)),
            ("mpn_candidate_count", size, (ptr,)),
            ("mpn_candidate", text, (ptr, size)),
            ("mpn_candidate_consumed", size, (ptr, size)),
            ("mpn_predict", size, (ptr, text, size)),
            ("mpn_prediction", text, (ptr, size)),
            ("mpn_text_no_suggest", number, (ptr, text)),
            ("mpn_english_open", ptr, (ctypes.c_void_p, size, number)),
            ("mpn_english_query_create", ptr, ()),
            ("mpn_english_complete", size, (ptr, ptr, text, text, ctypes.c_uint32)),
            ("mpn_english_correct", size, (ptr, ptr, text, text, ctypes.c_uint32)),
            ("mpn_english_predict", size, (ptr, ptr, text, ctypes.c_uint32)),
            ("mpn_english_nine_key", size, (ptr, ptr, text, text, ctypes.c_uint32)),
            ("mpn_english_text", text, (ptr, size))):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
    return core


class Lexicon:
    """The rows of native-pinyin.tsv and native-zhuyin.tsv: every text, and the marked words with the
    keys that spell them in full (letters without tones)."""

    def __init__(self, resources):
        self.texts = set()
        self.spelled = collections.defaultdict(set)   # marked text -> full spellings (pinyin letters, zhuyin keys)
        self.pinyin = {}                              # marked text -> (reading, bare) of its heaviest full pinyin row
        self.zhuyin = {}
        self.simplified = {}                          # marked text -> its Simplified text
        self.traditional = set()                      # marked texts that Traditional Pinyin output spells
        self.zhuyin_texts = set()                     # marked texts of Zhuyin rows (Zhuyin output is Traditional)
        self.heaviest = []                            # (weight, hant, hans) of unmarked words
        self.keep = set()                             # everyday words that hold a listed word (`k`: 印度支那)
        self.fewest = {}                              # Zhuyin keys without tones -> fewest syllables of a row
        anywhere, _, keep = blocked_terms._zh()
        for mode in ("pinyin", "zhuyin"):
            best = {}
            with open(resources / f"native-{mode}.tsv", encoding="utf-8") as rows:
                next(rows)
                for line in rows:
                    fields = line.rstrip("\n").split("\t")
                    if len(fields) < 5:
                        break  # the context table follows
                    bare, reading, hant, hans, weight = fields[:5]
                    self.texts.update((hant, hans))
                    if mode == "zhuyin":
                        n = sum(reading.count(k) for k in TONES_ZHUYIN)
                        self.fewest[bare] = min(self.fewest.get(bare, n), n)
                    if len(fields) == 6 and fields[5] == "k":
                        self.keep.update((hant, hans))
                    marked = len(fields) == 6 and fields[5] == "n"
                    full = mode == "zhuyin" or (bare.islower() and fully_spelled(reading))
                    if marked and full:
                        if mode == "pinyin":
                            self.traditional.add(hant)
                        else:
                            self.zhuyin_texts.add(hant)
                        for text in (hant, hans):
                            self.spelled[text].add(bare.lower())
                            self.simplified[text] = hans
                            if best.get(text, (-1,))[0] < int(weight):
                                best[text] = (int(weight), reading, bare)
                    elif mode == "pinyin" and full and not marked and 2 <= len(hans) <= 4 and bare.islower():
                        self.heaviest.append((int(weight), hant, hans))
            getattr(self, mode).update({text: (reading, bare) for text, (_, reading, bare) in best.items()})
        # A marked text inside another text counts only when it holds a term that matches anywhere
        # (a whole-word term, =term, is not volunteered by a longer word that happens to hold it).
        self.inside = {t for t, hans in self.simplified.items()
                       if len(t) >= 2 and hans not in keep and anywhere.search(hans)}
        self.longest = max(map(len, self.spelled), default=1)
        self.heaviest.sort(reverse=True)


def fully_spelled(reading):
    """A pinyin reading of whole syllables (not initials: t1a1m2)."""
    return all(re.search("[aeiouv]", s) for s in re.split("[1-5]", reading) if s)


def letters(keys, pinyin):
    if pinyin:
        return re.sub("[^a-z]", "", keys.lower())
    return "".join(k for k in keys if k not in TONES_ZHUYIN + "'")


class Checker:
    def __init__(self, lexicon, show):
        self.lex = lexicon
        self.found = collections.defaultdict(list)
        self.counts = collections.Counter()
        self.show = show

    def report(self, kind, keys, text):
        self.found[kind].append((keys, text))

    def candidate_volunteers(self, keys, text, pinyin, spell=None):
        """`keys`: the keys the candidate reads (its consumed part). `spell` turns a full spelling
        into keys (nine-key digits); by default the letters of the keys are compared."""
        typed = letters(keys, pinyin) if spell is None else keys.replace("'", "")
        spellings = lambda t: {spell(s) if spell else s for s in self.lex.spelled[t]} - {None}
        if text in self.lex.spelled:
            return typed not in spellings(text)
        if text in self.lex.texts:
            return False
        n, longest = len(text), self.lex.longest
        # A listed word inside an everyday word that holds it (密支那) is not looked for.
        kept = [(a, b) for a in range(n) for b in range(a + 3, n + 1) if text[a:b] in self.lex.keep]
        for a in range(n):
            for b in range(a + 2, min(n, a + longest) + 1):
                part = text[a:b]
                if any(ka <= a and b <= kb for ka, kb in kept):
                    continue
                if part in self.lex.inside and not any(s in typed for s in spellings(part)):
                    return True
        return False

    def prediction_volunteers(self, before, prediction):
        """The predictor reads the last three characters before the cursor."""
        if prediction in self.lex.spelled:
            return True
        tail = before[-3:]
        joined = tail + prediction
        n = len(joined)
        for a in range(len(tail)):
            for b in range(len(tail) + 1, min(n, a + self.lex.longest) + 1):
                if joined[a:b] in self.lex.inside or (a == 0 and joined[a:b] in self.lex.spelled and b - a == n):
                    return True
        return False


def type_keys(core, session, keys, each):
    """Types `keys` key by key into a cleared session; `each(prefix)` after every key."""
    core.mpn_clear(session)
    for end, key in enumerate(keys, 1):
        core.mpn_key(session, ord(key))
        each(keys[:end])


def candidates(core, session, top):
    count = min(core.mpn_candidate_count(session), top)
    return [core.mpn_candidate(session, i).decode("utf-8") for i in range(count)]


def read_candidates(core, session, top):
    """(text, input bytes it reads) of the first `top` candidates."""
    count = min(core.mpn_candidate_count(session), top)
    return [(core.mpn_candidate(session, i).decode("utf-8"), core.mpn_candidate_consumed(session, i)) for i in range(count)]


def keypad(spelling):
    """Nine-key digits of a Pinyin spelling (None for Zhuyin keys)."""
    if not (spelling.isascii() and spelling.isalpha()):
        return None
    return "".join(DIGITS[ord(c) - 97] for c in spelling.lower())


def initials_of(reading, pinyin):
    """Initials keys of a full reading: (first letters, with zh/ch/sh in full) or Zhuyin first symbols."""
    if pinyin:
        syllables = [s for s in re.split("[1-5]", reading) if s]
        short = "".join(s[0] for s in syllables)
        full = "".join(s[:2] if s[:2] in ("zh", "ch", "sh") else s[0] for s in syllables)
        mixed = syllables[0] + "".join(s[0] for s in syllables[1:])
        return syllables, {short, full, mixed} - {"".join(syllables)}
    syllables = [s for s in re.split("[" + re.escape(TONES_ZHUYIN) + "]", reading) if s]
    return syllables, {"".join(s[0] for s in syllables)} - {"".join(syllables)}


def typos(bare):
    """One-key typos and fuzzy sounds of plain letters, at most 12."""
    out = []
    n = len(bare)
    for i in (n // 2, n - 2, 1):
        if 0 < i < n:
            out.append(bare[:i] + bare[i + 1:])                       # a key dropped
            out.append(bare[:i] + bare[i] + bare[i:])                 # a key doubled
            out.append(bare[:i - 1] + bare[i] + bare[i - 1] + bare[i + 1:])  # two keys swapped
            near = NEIGHBOURS.get(bare[i], "")
            if near:
                out.append(bare[:i] + near[0] + bare[i + 1:])         # a neighbouring key
    out.append(re.sub("ng$", "n", bare) if bare.endswith("ng") else re.sub("n$", "ng", bare))
    out.append(re.sub("^(z|c|s)h", r"\1", bare) if re.match("^[zcs]h", bare) else re.sub("^([zcs])", r"\1h", bare))
    out.append(bare + "g" if bare.endswith("n") else bare + "n")
    return [t for t in dict.fromkeys(out) if t != bare][:12]


def check_chinese(core, base, resources, lex, checker, quick):
    py = core.mpn_lexicon_create_from_base(base, len(base), 1)
    pairs = ctypes.create_string_buffer((resources / "mimo_pairs.bin").read_bytes())
    english = ctypes.create_string_buffer((resources / "mimo_en.bin").read_bytes())
    if not core.mpn_lexicon_attach_pairs(py, pairs, len(pairs) - 1, None, None):
        sys.exit("check-no-volunteer: mimo_pairs.bin was not accepted")
    if not core.mpn_lexicon_attach_english(py, english, len(english) - 1, None, None):
        sys.exit("check-no-volunteer: mimo_en.bin was not accepted")
    zy = core.mpn_lexicon_create_from_base(base, len(base), 0)
    pool = json.loads((resources / "native-japanese.json").read_text(encoding="utf-8"))
    sessions = {}
    for script in "ts":
        session = core.mpn_session_create(py)
        core.mpn_set_simplified(session, 1 if script == "s" else 0)
        for reading, entries in pool.items():
            if entries:
                core.mpn_add_japanese_word(session, reading.encode(), entries[0]["text"].encode(), entries[0]["weight"])
        sessions[script] = session
    zhuyin = core.mpn_session_create(zy)
    nine = core.mpn_nine_key_session_create(py)
    core.mpn_set_simplified(nine, 1)
    hold = (pairs, english)

    def watch(session, pinyin, kind, top=TOP, spell=None):
        def each(keys):
            checker.counts[kind] += 1
            for text, consumed in read_candidates(core, session, top):
                if checker.candidate_volunteers(keys[:consumed], text, pinyin, spell):
                    checker.report(kind, keys, text)
        return each

    words = sorted(lex.pinyin, key=lambda t: (lex.simplified[t], t))
    by_script = {"s": [t for t in words if t == lex.simplified[t]], "t": [t for t in words if t in lex.traditional]}
    # Typed in full: no shorter input offers the word, and the full spelling still does.
    lost = []
    for script, session in sessions.items():
        for text in by_script[script]:
            reading, bare = lex.pinyin[text]
            spelling = bare.lower()
            type_keys(core, session, spelling, watch(session, True, "prefix (pinyin)"))
            if text not in candidates(core, session, EXACT_TOP) and not text.isascii():
                lost.append((spelling, text))
    for text in sorted(lex.zhuyin_texts):
        reading = lex.zhuyin[text][0]
        type_keys(core, zhuyin, reading, watch(zhuyin, False, "prefix (zhuyin)"))
        if text not in candidates(core, zhuyin, EXACT_TOP):
            lost.append((reading, text))
        # Only the last syllable's tone, as most people type Zhuyin; not where the same keys also read
        # as fewer syllables (ㄕㄨㄟˇ is 水): there the engine wants every tone, as the full spelling.
        last_tone = "".join(k for k in reading[:-1] if k not in TONES_ZHUYIN) + reading[-1]
        syllables = sum(reading.count(k) for k in TONES_ZHUYIN)
        if last_tone != reading and lex.fewest.get(letters(reading, False), syllables) >= syllables:
            type_keys(core, zhuyin, last_tone, lambda keys: None)
            if text not in candidates(core, zhuyin, EXACT_TOP):
                lost.append((last_tone, text))
    # Initials, Zhuyin first symbols, nine-key initials digits.
    for text in words:
        reading, bare = lex.pinyin[text]
        syllables, keys = initials_of(reading, True)
        if len(syllables) < 2:
            continue
        for script in "ts" if not quick else "s":
            for k in keys:
                type_keys(core, sessions[script], k, watch(sessions[script], True, "initials (pinyin)"))
        type_keys(core, nine, keypad("".join(s[0] for s in syllables)), watch(nine, True, "initials (nine-key)", spell=keypad))
        type_keys(core, nine, keypad(bare)[:-1], watch(nine, True, "prefix (nine-key)", spell=keypad))
    for text in sorted(lex.zhuyin):
        reading, _ = lex.zhuyin[text]
        syllables, keys = initials_of(reading, False)
        if len(syllables) >= 2:
            for k in keys:
                type_keys(core, zhuyin, k, watch(zhuyin, False, "initials (zhuyin)"))
    # One-key typos and fuzzy sounds.
    for text in words:
        _, bare = lex.pinyin[text]
        for typo in typos(bare.lower()):
            for script in "s" if quick else "ts":
                type_keys(core, sessions[script], typo, watch(sessions[script], True, "typos (pinyin)"))
    # Next-word associations.
    seeds = set()
    for _, hant, hans in lex.heaviest[:6000]:
        seeds.update((hant, hans))
    for text in lex.spelled:
        if not text.isascii():
            seeds.add(text)
            seeds.update(text[:i] for i in range(1, len(text)))
    for script, session in sessions.items():
        for seed in sorted(seeds):
            n = core.mpn_predict(session, seed.encode(), 16)
            checker.counts["predictions"] += 1
            for i in range(n):
                prediction = core.mpn_prediction(session, i).decode("utf-8")
                if checker.prediction_volunteers(seed, prediction):
                    checker.report("predictions", seed, prediction)
    # The engine's own answer for client filtering.
    for text in sorted(lex.spelled):
        if not core.mpn_text_no_suggest(sessions["s"], text.encode()):
            checker.report("mpn_text_no_suggest", text, "not marked")
    checker.counts["mpn_text_no_suggest"] += len(lex.spelled)
    # Every [zh] term is a marked word of its own (a whole-word term, =term, is marked `w` and is
    # not looked for inside other text, so it is not counted here).
    terms = [t for t in blocked_terms.sections()["zh"] if not t.startswith("=") and len(t) >= 2]
    for term in terms:
        if term not in lex.spelled:
            checker.report("list terms with rows", term, "no marked row")
    checker.counts["list terms with rows"] += len(terms)
    return lost, hold


def check_english(core, resources, checker):
    data = (resources / "mimo_en.bin").read_bytes()
    buffer = ctypes.create_string_buffer(data)
    english = core.mpn_english_open(buffer, len(data), 1)
    query = core.mpn_english_query_create()
    listed = blocked_terms.en_words()

    def results(n):
        return [core.mpn_english_text(query, i).decode("utf-8") for i in range(n)]

    def key(word):
        return re.sub("[^a-z]", "", word.lower())

    def volunteered(items, typed, digits=False):
        """Listed words among `items` that `typed` does not spell in full."""
        spelled = lambda w: (keypad(key(w)) if digits else key(w)) == typed.lower()
        return [w for w in items if blocked_terms.en_hit(w) and not spelled(w)]

    for word in sorted(listed):
        if not word.isalpha():
            continue
        for i in range(1, len(word)):
            prefix = word[:i]
            checker.counts["english completions"] += 1
            for w in volunteered(results(core.mpn_english_complete(query, english, prefix.encode(), None, 0)), prefix):
                checker.report("english completions", prefix, w)
            digits = "".join(DIGITS[ord(c) - 97] for c in prefix)
            checker.counts["english nine-key"] += 1
            for w in volunteered(results(core.mpn_english_nine_key(query, english, digits.encode(), None, 0)), digits, True):
                checker.report("english nine-key", digits, w)
        for typo in typos(word):
            checker.counts["english corrections"] += 1
            for w in volunteered(results(core.mpn_english_correct(query, english, typo.encode(), None, 0)), typo):
                checker.report("english corrections", typo, w)
    reader = module("mimo_en_binary", ROOT / "scripts/mimo_en_binary.py").Reader(data)
    heaviest = sorted(range(reader.n), key=lambda r: -reader.weight(r))[:2000]
    pairs = blocked_terms.en_pairs()
    seeds = [reader.spelling(r) for r in heaviest] + sorted(listed) + sorted({l for l, _ in pairs}) + [""]
    for seed in seeds:
        checker.counts["english next words"] += 1
        before = (seed + " ").encode() if seed else None
        offered = results(core.mpn_english_predict(query, english, before, 0))
        for w in volunteered(offered, ""):
            checker.report("english next words", seed, w)
        for w in offered:
            if (blocked_terms.plain_form(seed), blocked_terms.plain_form(w)) in pairs:
                checker.report("english next words", seed, w)
    return buffer


def check_emoji(resources, checker):
    emoji = module("build_emoji_associations", ROOT / "scripts/build-emoji-associations.py")
    table = json.loads((resources / "associations.json").read_text(encoding="utf-8"))
    for lang, entries in table.items():
        for word, entry in entries.items():
            checker.counts["emoji"] += 1
            if emoji.never_offered(lang, word):
                checker.report("emoji", f"{lang} key", word)
            for symbol in entry[0]:
                if emoji.banned(symbol):
                    checker.report("emoji", f"{lang} {word}", " ".join(f"U+{ord(c):04X}" for c in symbol))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--lib", default=str(ROOT / "target/release/libmeowpuut_ffi.dylib"))
    parser.add_argument("--quick", action="store_true", help="initials and typos in the Simplified session only")
    parser.add_argument("--show", type=int, default=5, help="examples shown per kind (counts only with 0)")
    args = parser.parse_args()
    resources = Path(args.resources)
    core = load_core(args.lib)
    lex = Lexicon(resources)
    checker = Checker(lex, args.show)
    base = (resources / "mimo_base.bin").read_bytes()
    lost, hold = check_chinese(core, base, resources, lex, checker, args.quick)
    hold_en = check_english(core, resources, checker)
    check_emoji(resources, checker)
    del hold, hold_en
    print(f"marked words: {len(lex.spelled)} texts ({len(lex.pinyin)} with a Pinyin spelling, {len(lex.zhuyin)} Zhuyin); "
          f"[en] words: {len(blocked_terms.en_words())}")
    failed = False
    for kind in sorted(set(checker.counts) | set(checker.found)):
        found = checker.found.get(kind, [])
        print(f"{kind:24} {checker.counts[kind]:8} checked  {len(found):5} volunteered")
        failed |= bool(found)
        for keys, text in found[:args.show]:
            print(f"    {keys!r} -> {text}")
    print(f"{'typed in full':24} {len(lex.pinyin) + len(lex.zhuyin_texts):8} words    {len(lost):5} not in the first {EXACT_TOP}")
    for keys, text in lost[:args.show]:
        print(f"    {keys!r} -> {text} missing")
    failed |= bool(lost)
    print("FAIL" if failed else "PASS: nothing on the list is volunteered; every marked word is typed by its full spelling")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
