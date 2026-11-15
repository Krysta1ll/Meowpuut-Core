#!/usr/bin/env python3
"""Whole-sentence conversion accuracy of the Pinyin decoder (ranking / word-frequency quality).

  python3 scripts/eval-ranking.py [FILE] [--lib PATH] [--resources DIR] [--no-pairs]
                                  [--show-failures N] [--dump OUT.tsv] [--compare OLD.tsv]

FILE (default Tests/eval/heldout-sentences.tsv) has one sentence per line:
    script<TAB>sentence<TAB>pinyin syllables separated by spaces, no tones
script is t (Traditional) or s (Simplified); lines starting with # are comments.

Every line gets a fresh session with the script set and the mixed Japanese pool
registered the way the clients do it (as in scripts/check-chinese-golden.py).
The pinyin letters are typed key by key. Then the script behaves like a user
who only ever takes the first candidate: mpn_choose(0) commits candidate 1
(mpn_committed holds the text of that pick); if the candidate covered only part
of the input, the rest stays in mpn_input and the next first candidate is taken,
up to 20 picks. The commits are concatenated.

Metrics (per script and overall):
  sentence  produced == sentence
  char      1 - edit distance / len(sentence), averaged over characters
  top1      candidate 1 right after typing is the whole sentence
  picks     first-candidate picks needed (1 = the decoder segmented it in one go)
Scoring is strict: variant forms (裡/裏, 着/著) count as errors. Failures whose
produced text is one edit away from the sentence are counted separately.

--dump writes per-sentence results as TSV; --compare OLD.tsv lists sentences
that became right or wrong against such a dump. Without --lib the release
library target/release/libmeowpuut_ffi.* is used (it is never built here).
"""
import argparse
import ctypes
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAX_PICKS = 20


def golden_module():
    """scripts/check-chinese-golden.py, imported for its ctypes loader."""
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("check_chinese_golden", ROOT / "scripts/check-chinese-golden.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def default_lib():
    ext = "dylib" if sys.platform == "darwin" else ("dll" if sys.platform == "win32" else "so")
    prefix = "" if sys.platform == "win32" else "lib"
    return ROOT / "target/release" / f"{prefix}meowpuut_ffi.{ext}"


def load_core(lib):
    core = golden_module().load_core(str(lib))
    ptr, text, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t
    for name, result, arguments in (
            ("mpn_choose", ctypes.c_int, (ptr, size)),
            ("mpn_input", text, (ptr,)),
            ("mpn_preedit", text, (ptr,)),
            ("mpn_clear_commit", None, (ptr,)),
            ("mpn_candidate_consumed", size, (ptr, size)),
            ("mpn_max_input_bytes", size, ()),
            ("mpn_lexicon_attach_pairs", ctypes.c_int, (ptr, text, size, ptr, ptr)),
            ("mpn_lexicon_attach_english", ctypes.c_int, (ptr, text, size, ptr, ptr))):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
    return core


def read_sentences(path):
    rows = []
    for number, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 3 or fields[0] not in ("t", "s"):
            sys.exit(f"eval-ranking: {path}:{number}: expected script<TAB>sentence<TAB>pinyin")
        rows.append((fields[0], fields[1].strip(), " ".join(fields[2].split())))
    return rows


def edit_distance(a, b):
    previous = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        current = [i]
        for j, y in enumerate(b, 1):
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (x != y)))
        previous = current
    return previous[-1]


class Engine:
    def __init__(self, core, resources, pairs, english=False):
        self.core = core
        self.base = (resources / "mimo_base.bin").read_bytes()
        self.lexicon = core.mpn_lexicon_create_from_base(self.base, len(self.base), 1)
        if not self.lexicon:
            sys.exit("eval-ranking: mimo_base.bin did not load")
        self.pairs = "off"
        path = resources / "mimo_pairs.bin"
        if pairs and path.exists():
            # Kept alive for the process: release is NULL, so the bytes must outlive the lexicon.
            self.pair_bytes = ctypes.create_string_buffer(path.read_bytes())
            size = len(self.pair_bytes) - 1
            self.pairs = "on" if core.mpn_lexicon_attach_pairs(self.lexicon, self.pair_bytes, size, None, None) \
                else "rejected (not made for this lexicon)"
        self.english = "off"
        english_path = resources / "mimo_en.bin"
        if english:
            if not english_path.exists():
                sys.exit("eval-ranking: --english needs mimo_en.bin in the resources directory")
            # English words inside Pinyin, as the clients attach them.
            self.english_bytes = ctypes.create_string_buffer(english_path.read_bytes())
            self.english = "on" if core.mpn_lexicon_attach_english(
                self.lexicon, self.english_bytes, len(self.english_bytes) - 1, None, None) else "rejected"
        pool = json.loads((resources / "native-japanese.json").read_text(encoding="utf-8"))
        self.pool = [(reading.encode(), entries[0]["text"].encode(), entries[0]["weight"])
                     for reading, entries in pool.items() if entries]
        self.limit = core.mpn_max_input_bytes()

    def session(self, script):
        core = self.core
        session = core.mpn_session_create(self.lexicon)
        core.mpn_set_simplified(session, 1 if script == "s" else 0)
        for reading, text, weight in self.pool:
            core.mpn_add_japanese_word(session, reading, text, weight)
        return session

    def convert(self, script, pinyin):
        """(produced text, picks, candidate 1 after typing, note)."""
        core = self.core
        keys = pinyin.replace(" ", "")
        note = ""
        if len(keys) > self.limit:
            note = f"input longer than {self.limit} bytes"
        session = self.session(script)
        produced = []
        try:
            for key in keys:
                core.mpn_key(session, ord(key))
                done = core.mpn_committed(session)
                if done:  # input limit reached: the old composition went out as it was
                    produced.append(done.decode("utf-8"))
                    core.mpn_clear_commit(session)
                    note = note or "committed while typing"
            count = core.mpn_candidate_count(session)
            first = core.mpn_candidate(session, 0).decode("utf-8") if count else ""
            picks = 0
            while (core.mpn_input(session) or b"") and picks < MAX_PICKS:
                if not core.mpn_candidate_count(session):
                    produced.append(core.mpn_input(session).decode("utf-8"))
                    note = note or "no candidate for the rest"
                    break
                if not core.mpn_choose(session, 0):
                    note = note or "mpn_choose failed"
                    break
                picks += 1
                produced.append((core.mpn_committed(session) or b"").decode("utf-8"))
                core.mpn_clear_commit(session)
            else:
                if core.mpn_input(session):
                    produced.append(core.mpn_input(session).decode("utf-8"))
                    note = note or f"input left after {MAX_PICKS} picks"
        finally:
            core.mpn_session_destroy(session)
        return "".join(produced), picks, first, note


def evaluate(engine, rows):
    results = []
    for script, sentence, pinyin in rows:
        produced, picks, first, note = engine.convert(script, pinyin)
        results.append({
            "script": script, "sentence": sentence, "pinyin": pinyin, "produced": produced,
            "picks": picks, "top1": int(first == sentence), "ok": int(produced == sentence),
            "edits": edit_distance(sentence, produced), "first": first, "note": note})
    return results


def summarise(results):
    groups = [("t", [r for r in results if r["script"] == "t"]),
              ("s", [r for r in results if r["script"] == "s"]), ("all", results)]
    header = f"{'script':<7}{'n':>6}{'sentence':>10}{'char':>9}{'top1':>9}{'picks':>8}{'1-pick':>9}{'fail':>6}{'1-edit':>8}"
    lines = [header]
    for name, group in groups:
        if not group:
            continue
        n = len(group)
        chars = sum(len(r["sentence"]) for r in group)
        errors = sum(min(r["edits"], len(r["sentence"])) for r in group)
        fails = [r for r in group if not r["ok"]]
        lines.append(f"{name:<7}{n:>6}"
                     f"{100 * sum(r['ok'] for r in group) / n:>9.1f}%"
                     f"{100 * (1 - errors / max(chars, 1)):>8.1f}%"
                     f"{100 * sum(r['top1'] for r in group) / n:>8.1f}%"
                     f"{sum(r['picks'] for r in group) / n:>8.2f}"
                     f"{100 * sum(r['picks'] == 1 for r in group) / n:>8.1f}%"
                     f"{len(fails):>6}{sum(r['edits'] == 1 for r in fails):>8}")
    lines.append("sentence = exact match; char = 1 - edits/chars; top1 = candidate 1 after typing is the sentence;")
    lines.append("picks = mean first-candidate picks; 1-pick = done in one pick; 1-edit = failures one character away.")
    return "\n".join(lines)


DUMP_FIELDS = ("script", "sentence", "pinyin", "ok", "top1", "picks", "edits", "produced", "first", "note")


def dump(results, path):
    with open(path, "w", encoding="utf-8") as out:
        out.write("\t".join(DUMP_FIELDS) + "\n")
        for r in results:
            out.write("\t".join(str(r[field]).replace("\t", " ") for field in DUMP_FIELDS) + "\n")


def load_dump(path):
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    fields = lines[0].split("\t")
    rows = {}
    for line in lines[1:]:
        row = dict(zip(fields, line.split("\t")))
        rows[(row["script"], row["sentence"], row["pinyin"])] = row
    return rows


def compare(results, path):
    old = load_dump(path)
    fixed, broken, top_fixed, top_broken, changed = [], [], 0, 0, 0
    for r in results:
        before = old.get((r["script"], r["sentence"], r["pinyin"]))
        if before is None:
            continue
        was, now = before["ok"] == "1", bool(r["ok"])
        if was and not now:
            broken.append((r, before))
        elif now and not was:
            fixed.append((r, before))
        elif not now and before["produced"] != r["produced"]:
            changed += 1
        top_fixed += before["top1"] == "0" and r["top1"] == 1
        top_broken += before["top1"] == "1" and r["top1"] == 0
    common = sum(1 for r in results if (r["script"], r["sentence"], r["pinyin"]) in old)
    print(f"\ncompare with {path}: {common} sentences in both")
    print(f"  became right {len(fixed)}, became wrong {len(broken)} (net {len(fixed) - len(broken):+d}); "
          f"top1 +{top_fixed} -{top_broken}; still wrong but different output {changed}")
    for title, items in (("became right", fixed), ("became wrong", broken)):
        if items:
            print(f"  {title}:")
        for r, before in items:
            print(f"    [{r['script']}] {r['sentence']} | was {before['produced']} | now {r['produced']} | {r['pinyin']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("file", nargs="?", default=str(ROOT / "Tests/eval/heldout-sentences.tsv"))
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--lib", default=None)
    parser.add_argument("--no-pairs", action="store_true", help="do not attach mimo_pairs.bin")
    parser.add_argument("--english", action="store_true", help="attach mimo_en.bin (English inside Pinyin, as the clients do)")
    parser.add_argument("--show-failures", type=int, default=0, metavar="N")
    parser.add_argument("--dump", metavar="PATH")
    parser.add_argument("--compare", metavar="OLD")
    arguments = parser.parse_args()
    lib = Path(arguments.lib) if arguments.lib else default_lib()
    if not lib.exists():
        sys.exit(f"eval-ranking: {lib} not found (build it with cargo build --release -p meowpuut-ffi)")
    rows = read_sentences(arguments.file)
    engine = Engine(load_core(lib), Path(arguments.resources), not arguments.no_pairs, arguments.english)
    results = evaluate(engine, rows)
    print(f"{arguments.file}: {len(rows)} sentences; pairs {engine.pairs}; english {engine.english}")
    print(summarise(results))
    notes = [r for r in results if r["note"]]
    if notes:
        print(f"{len(notes)} sentences with notes, e.g. {notes[0]['sentence']}: {notes[0]['note']}")
    if arguments.show_failures:
        failures = [r for r in results if not r["ok"]]
        print(f"\nfailures ({min(len(failures), arguments.show_failures)} of {len(failures)}): "
              "sentence | produced | pinyin")
        for r in failures[:arguments.show_failures]:
            mark = " (1 edit)" if r["edits"] == 1 else ""
            print(f"  [{r['script']}] {r['sentence']} | {r['produced']} | {r['pinyin']}  picks={r['picks']}{mark}")
    if arguments.dump:
        dump(results, arguments.dump)
        print(f"\nwrote {arguments.dump}")
    if arguments.compare:
        compare(results, arguments.compare)
    return 0


if __name__ == "__main__":
    sys.exit(main())
