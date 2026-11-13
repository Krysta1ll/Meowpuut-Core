#!/usr/bin/env python3
"""Checks of the mimo-jp snapshot and the compiled mimo_jp.bin (design tests T4 and T11).

usage: check-mimo-jp.py [DIR] [--allow-hold] [--source-only] [--probe] [--lib PATH]

DIR is a compiled directory holding mimo_jp.bin, mimo_base.bin, native-pinyin.tsv and
native-japanese.json (default: .build/lexicon).
--source-only  checks that need no compiled output: SOURCE.json hashes, deny file, kanji table,
               the Python romaji port.
--allow-hold   accept `hold` rows in Resources/Lexicon/japanese-deny.tsv.
--probe        also print the T4 numbers and fail when they are over budget.
--lib PATH     libmeowpuut_ffi to use (default: target/release, built with cargo first).
Exit status 0 when every check passes.
"""
import collections
import ctypes
import hashlib
import json
import math
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from japanese_readings import _check as romaji_check, load_kanji_table, normalise_key  # noqa: E402
from mimo_jp_binary import (FLAG_PINYIN_OK, FLAG_REVIEWED, KIND_ALIAS_KANA, KIND_ALIAS_KANJI,  # noqa: E402
                            KIND_AUTHORED, KIND_READING, REVIEWED_POOL, chinese_build_of, posting_order,
                            read_image)
from mimo_jp_compile import (MAX_POOL, MIN_LETTERS, MIN_PINYIN_LETTERS, W_COLLIDE, W_PINYIN,  # noqa: E402
                             ChineseSide, load_deny)

ROOT = Path(__file__).resolve().parent.parent
MAX_IMAGE = 4 * 1024 * 1024

# Expected first candidates and romaji-over-alias pairs of check-japanese-lexicon.py.
FIRST = {
    'oshi': '推し', 'oshikatsu': '推し活', 'tuishihuo': '推し活', 'seiyuu': '声優', 'seiyu': '声優',
    'doujinshi': '同人誌', 'haishin': '配信', 'douga': '動画', 'kashi': '歌詞',
    'kashu': '歌手', 'onsei': '音声', 'utattemita': '歌ってみた', 'utaite': '歌い手',
    'sakuhin': '作品', 'monogatari': '物語', 'kyara': 'キャラ', 'kyarakutaa': 'キャラクター',
    'sutoorii': 'ストーリー', 'ningen': '人間', 'egao': '笑顔', 'yasashii': '優しい',
    'atsui': '暑い', 'samui': '寒い', 'issho': '一緒', 'minna': 'みんな',
    'zutto': 'ずっと', 'mata': 'また', 'tabun': '多分', 'mochiron': 'もちろん',
    'kitto': 'きっと', 'shiawase': '幸せ', 'taihen': '大変', 'kirakira': 'キラキラ',
    'dokidoki': 'ドキドキ', 'sukoshi': '少し', 'chigau': '違う', 'sou': 'そう',
    'bangumi': '番組', 'karaoke': 'カラオケ',
}
OVER_ALIAS = [('suki', 'haoki', '好き'), ('oshikatsu', 'tuishihuo', '推し活'),
              ('taberu', 'shiberu', '食べる'), ('nomu', 'yinmu', '飲む'),
              ('iku', 'xingku', '行く'), ('kawaii', 'keaii', '可愛い'),
              ('ongaku', 'yinyue', '音楽'), ('sanpo', 'sanbu', '散歩')]
# The 29 words the research run expected (research.md), all of which must be in the image.
EXPECTED = ('推し 推し活 声優 同人誌 配信 動画 歌詞 歌い手 物語 キャラクター 笑顔 優しい 一緒 みんな ずっと 多分 '
            'もちろん 幸せ 大変 キラキラ ドキドキ 番組 カラオケ 寿司 猫 日本語 東京 お疲れ様 美味しい').split()
# The 42 Chinese sentences of crates/meowpuut-core/tests/jp_false_positive_probe.rs (design T5).
SENTENCES = """womenjintianqukandianying jintiantianqihenhao nihaowoshizhongguoren zhegewentihennanjiejue
mingtianyiqiquchifanba xiexienidebangzhu wozaixuexiriyu nizuijinzenmeyang wojuedezhegebucuo tamenyijingzoule
qingwenxishoujianzainali wanshangyaobuyaoyiqichifan woshinanren tashinvren womendoushihaopengyou
zhejianshiqinghenzhongyao woxiangmaiyibenshu nikanguozhebudianyingma wojintianyoudianmang
zuotianwanshangwomeishuihao tadehanyushuodehenhao qingdengyixia womenmingtianjian duibuqiwolaiwanle
zhegeduoshaoqian wobuzhidaozenmeban nizaiganshenme xianzaijidianle wohenxihuanzheshouge qingbangwokaiyixiamen
shijieshanghaiyouhenduoren womenyiqinuliba zhongwenhennandanshihenyouyisi tamendoushidaxuesheng woyaoyibeikafei
jintianshixingqiji womingtianyaoshangban nichifanlema wanan zaijian meiguanxi bukeqi""".split()
# Budgets of design section 12.
BUDGET_KEYSTROKES = 0.8
BUDGET_WEIGHTED = 1.3
BUDGET_SENTENCES = 1.0
BUDGET_COMPLETE = 120

failures = []


def check(condition, message):
    if not condition:
        failures.append(message)
    return condition


def source_checks(allow_hold):
    source = ROOT / "DataSources/mimo-jp"
    manifest = json.loads((source / "SOURCE.json").read_text())
    for name, expected in manifest["files"].items():
        check(hashlib.sha256((source / name).read_bytes()).hexdigest() == expected, f"SOURCE.json: {name} differs")
    deny = load_deny(ROOT / "Resources/Lexicon/japanese-deny.tsv")
    holds = sum(1 for reason in deny.values() if reason == "hold")
    check(allow_hold or holds == 0, f"japanese-deny.tsv: {holds} hold rows")
    snapshot = {}
    for line in (source / "mimo-jp.tsv").read_text(encoding="utf-8").splitlines():
        if not line.startswith("#"):
            fields = line.split("|")
            snapshot[(fields[0], fields[1])] = fields
    check(not any("〆" in surface for _, surface in snapshot), "a surface contains 〆")
    unmatched = [pair for pair in deny if pair not in snapshot]
    check(not unmatched, f"japanese-deny.tsv: {len(unmatched)} rows match no snapshot row, e.g. {unmatched[:3]}")
    table = load_kanji_table(ROOT / "Resources/Lexicon/japanese-kanji-pinyin.tsv")
    report = json.loads((source / "build-report.json").read_text())
    missing = [item["kanji"] for item in report["review"]["polyphones"] if item["words"] >= 20 and item["kanji"] not in table]
    check(not missing, f"polyphonic kanji in 20 words or more without a table line: {''.join(missing)}")
    status = romaji_check(str(source / "mimo-jp.tsv"))
    check(status == 0, "japanese_readings.py --check failed")
    print(f"source: {len(snapshot)} snapshot rows, {len(deny)} deny rows ({holds} hold), {len(table)} table lines")
    return deny, snapshot


def load_lib(path):
    if path is None:
        name = {"darwin": "libmeowpuut_ffi.dylib", "win32": "meowpuut_ffi.dll"}.get(sys.platform, "libmeowpuut_ffi.so")
        path = ROOT / "target/release" / name
        if True:  # as check-mixed-core.py: build so the library matches the sources
            env = {**os.environ, "PATH": f"{Path.home() / '.cargo/bin'}:{os.environ.get('PATH', '')}"}
            subprocess.run(["cargo", "build", "--release", "-p", "meowpuut-ffi"], cwd=ROOT, env=env, check=True)
    core = ctypes.CDLL(str(path))

    def sig(name, result, *arguments):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
        return function
    p, s, z = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t
    return {
        "open": sig("mpn_japanese_open", p, s, z, ctypes.c_int),
        "close": sig("mpn_japanese_close", None, p),
        "words": sig("mpn_japanese_word_count", z, p),
        "build": sig("mpn_japanese_chinese_build", ctypes.c_uint32, p),
        "query": sig("mpn_japanese_query_create", p),
        "lookup": sig("mpn_japanese_lookup", z, p, p, s, ctypes.c_int, z),
        "text": sig("mpn_japanese_text", s, p, z),
        "info": sig("mpn_japanese_info", ctypes.c_int, p, z, ctypes.c_void_p, z),
        "lexicon": sig("mpn_lexicon_create", p, s, z),
        "session": sig("mpn_session_create", p, p),
        "mixed": sig("mpn_session_add_japanese_mixed", z, p, p),
    }


class Info(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_uint32), ("flags", ctypes.c_uint32), ("weight", ctypes.c_int32), ("word", ctypes.c_uint32)]


class Lookup:
    def __init__(self, lib, data):
        self.lib, self.data = lib, data
        self.handle = lib["open"](data, len(data), 1)
        self.query = lib["query"]()

    def __call__(self, text, mode, limit=64):
        count = self.lib["lookup"](self.query, self.handle, text.encode(), mode, limit)
        rows = []
        for index in range(count):
            info = Info()
            self.lib["info"](self.query, index, ctypes.byref(info), ctypes.sizeof(info))
            rows.append((self.lib["text"](self.query, index).decode(), info.kind, info.flags, info.weight, info.word))
        return rows


def image_checks(directory, deny, lib_path, probe):
    data = (directory / "mimo_jp.bin").read_bytes()
    image = read_image(data)
    words, postings = image["words"], image["postings"]
    print("image:", len(data), "bytes;", ", ".join(f"{name} {size}" for name, size in image["sections"].items()))
    check(len(data) <= MAX_IMAGE, f"image is {len(data)} bytes")
    check(40000 <= len(words) <= 65000, f"word count {len(words)}")
    base = directory / "mimo_base.bin"
    check(base.exists() and image["chinese_build"] == chinese_build_of(base.read_bytes()),
          "chinese_build does not match mimo_base.bin of this directory")
    # Structure, as JapaneseLexicon::verify checks it.
    keys = [key for key, _, _ in image["keys"]]
    check(all(a.encode() < b.encode() for a, b in zip(keys, keys[1:])), "keys not strictly ascending")
    by_key = collections.defaultdict(list)
    for posting in postings:
        by_key[posting["key"]].append(posting)
    for key, group in by_key.items():
        surfaces = [words[p["word"]]["surface"] for p in group]
        check(len(set(surfaces)) == len(surfaces), f"surface twice under {key}")
        check(group == sorted(group, key=lambda p: posting_order(p, words)), f"postings of {key} out of order")
        check(len(key.encode()) <= 96 and len(group) <= 64, f"key {key} over a limit")
    word_weight = {}
    for posting in postings:
        if posting["kind"] == KIND_READING:
            word_weight[posting["word"]] = posting["weight"]

    lib = load_lib(lib_path)
    look = Lookup(lib, data)
    check(look.handle is not None and lib["words"](look.handle) == len(words), "mpn_japanese_open failed")
    check(lib["build"](look.handle) == image["chinese_build"], "mpn_japanese_chinese_build differs")
    JAPANESE, PINYIN = 0, 1
    for reading, expected in FIRST.items():
        rows = look(reading, JAPANESE)
        check(rows and rows[0][0] == expected, f"{reading}: first {rows[0][0] if rows else None}, want {expected}")
    for standard, alias, expected in OVER_ALIAS:
        a = next((r[3] for r in look(standard, JAPANESE) if r[0] == expected), None)
        b = next((r[3] for r in look(alias, JAPANESE) if r[0] == expected), None)
        check(a is not None and b is not None and a > b, f"{standard} {a} must outweigh {alias} {b} for {expected}")
    surfaces = {word["surface"] for word in words}
    missing = [word for word in EXPECTED if word not in surfaces]
    check(not missing, f"expected words missing: {missing}")

    legacy = json.loads((directory / "native-japanese.json").read_text())
    reviewed = {row["reading"]: row for row in image["reviewed"]}
    check(set(reviewed) == set(legacy), "reviewed section keys differ from native-japanese.json")
    for reading, entries in legacy.items():
        row = reviewed.get(reading)
        check(row and row["text"] == entries[0]["text"] and row["weight"] == entries[0]["weight"],
              f"reviewed row {reading} differs from native-japanese.json")
    pool = sum(1 for row in image["reviewed"] if row["flags"] & REVIEWED_POOL)
    check(pool <= MAX_POOL, f"{pool} pool rows")
    native = (directory / "native-pinyin.tsv").read_bytes()
    lexicon = lib["lexicon"](native, len(native))
    session = lib["session"](lexicon)
    accepted = lib["mixed"](session, look.handle)
    check(accepted == pool, f"mpn_session_add_japanese_mixed accepted {accepted} of {pool}")

    # Deny pairs: none in the image, except reason `pinyin`, which stays without the pinyin bit.
    pinyin_words = {p["word"] for p in postings if p["flags"] & FLAG_PINYIN_OK}
    word_of_pair = {(p["key"], words[p["word"]]["surface"]): p["word"] for p in postings if p["kind"] == KIND_READING}
    for (kana, surface), reason in deny.items():
        pair = (normalise_key(kana), surface)
        if reason == "pinyin":
            check(word_of_pair.get(pair) not in pinyin_words or words[word_of_pair[pair]]["flags"] & FLAG_REVIEWED,
                  f"deny pinyin {kana}|{surface} is offered in pinyin mode")
        else:
            reviewed_word = pair in word_of_pair and words[word_of_pair[pair]]["flags"] & FLAG_REVIEWED
            check(pair not in word_of_pair or reviewed_word, f"denied {kana}|{surface} is in the image")

    # Gate, exhaustive (T11): every Chinese reading and every prefix of the heavy ones.
    rows = []
    for line in native.decode().splitlines()[1:]:
        fields = line.split("\t")
        if len(fields) != 5:
            break
        if fields[0][:1].isupper():
            continue  # English words (upper-case keys, crates/meowpuut-core/src/latin.rs) are no Chinese reading
        rows.append(((fields[0], fields[1], fields[2], fields[3]), int(fields[4])))
    side = ChineseSide(rows)
    heavy = sorted(((w, k) for k, w in side.zc.items() if w >= 4000 and len(k) >= 2), reverse=True)
    inputs = set(side.zc) | {k[:i] for _, k in heavy for i in range(1, len(k) + 1)}
    results = {}
    bad = collections.Counter()
    examples = []
    for text in sorted(inputs):
        if not text.isascii() or not text.isalpha():
            continue
        found = look(text, PINYIN, 5)
        results[text] = found
        for surface, kind, flags, weight, word in found:
            if flags & FLAG_REVIEWED:
                continue
            w = word_weight.get(word, 0)
            zc = side.zc.get(text, 0)
            problems = []
            if kind != KIND_ALIAS_KANJI:
                if len(text) < MIN_LETTERS or text in side.syllables:
                    problems.append("short or one syllable")
                if zc > 0 and w < W_COLLIDE:
                    problems.append("light word on a Chinese reading")
                if zc > 0 and kind in (KIND_READING,) and surface in side.zw[text]:
                    problems.append("twin")
                if side.pinyin_like(text) and (w < W_PINYIN or len(text) < MIN_PINYIN_LETTERS):
                    problems.append("pinyin-like")
                if kind == KIND_ALIAS_KANA and zc > 0:
                    problems.append("kana alias equals a Chinese reading")
            elif len(text) < MIN_PINYIN_LETTERS or w < W_PINYIN:
                problems.append("kanji alias short or light")
            for problem in problems:
                bad[problem] += 1
                if len(examples) < 10:
                    examples.append(f"{text} {surface} kind {kind} weight {w}: {problem}")
    check(not bad, f"gate: {dict(bad)}; {examples}")
    print(f"gate: {len(results)} pinyin-mode lookups, {sum(1 for r in results.values() if r)} with a row")
    if probe:
        probe_numbers(side, heavy, look, word_weight, PINYIN)


def slot_word(text, found, side):
    """The derived word the collapsed row would show, or None (design section 8, plus the owner rule
    that keeps a pure-kanji alias out of the collapsed row when the input is a Chinese reading)."""
    for surface, kind, flags, _weight, _word in found:
        if flags & FLAG_REVIEWED:
            return None
        if kind == KIND_ALIAS_KANJI and side.zc.get(text, 0) > 0:
            continue
        return surface
    return None


def probe_numbers(side, heavy, look, word_weight, mode):
    cache = {}

    def hit(text):
        if text not in cache:
            cache[text] = slot_word(text, look(text, mode, 5), side) is not None
        return cache[text]
    keystrokes = hits = 0
    weighted_total = weighted_hits = 0.0
    complete = []
    late = 0
    for w, reading in heavy:
        weight = math.exp(w / 700)
        for end in range(1, len(reading) + 1):
            keystrokes += 1
            weighted_total += weight
            if hit(reading[:end]):
                hits += 1
                weighted_hits += weight
        if hit(reading):
            complete.append(reading)
        if any(kind == KIND_ALIAS_KANJI for _, kind, flags, _, _ in look(reading, mode, 5) if not flags & FLAG_REVIEWED):
            late += 1
    sentence_keys = sum(len(s) for s in SENTENCES)
    sentence_hits = sum(1 for s in SENTENCES for end in range(1, len(s) + 1) if hit(s[:end]))
    share, weighted = 100 * hits / keystrokes, 100 * weighted_hits / weighted_total
    sentences = 100 * sentence_hits / sentence_keys
    print(f"probe: Chinese entries >= 4000: {len(heavy)} readings, {keystrokes} keystrokes, slot shows a derived word on "
          f"{hits} ({share:.2f}%; weighted {weighted:.2f}%); complete readings with a derived word in the slot: {len(complete)}")
    print(f"probe: 42 sentences: {sentence_keys} keystrokes, {sentence_hits} hits ({sentences:.2f}%)")
    print(f"probe: complete readings with a pure-kanji alias after the Chinese candidates (not in the slot): {late}")
    check(share <= BUDGET_KEYSTROKES and weighted <= BUDGET_WEIGHTED, f"T4: {share:.2f}% / {weighted:.2f}% over budget")
    check(sentences <= BUDGET_SENTENCES, f"T4: sentences {sentences:.2f}% over budget")
    check(len(complete) <= BUDGET_COMPLETE, f"T4: {len(complete)} complete readings over budget")


def main(argv):
    args = list(argv)
    flags = {flag for flag in ("--allow-hold", "--source-only", "--probe") if flag in args}
    lib_path = None
    if "--lib" in args:
        lib_path = Path(args[args.index("--lib") + 1])
        del args[args.index("--lib"):args.index("--lib") + 2]
    rest = [arg for arg in args if arg not in flags]
    if any(arg.startswith("-") for arg in rest):
        print(__doc__.strip())
        return 2
    directory = Path(rest[0]) if rest else ROOT / ".build/lexicon"
    deny, _ = source_checks("--allow-hold" in flags)
    if "--source-only" not in flags:
        image_checks(directory, deny, lib_path, "--probe" in flags)
        licenses = directory / "Licenses"
        if licenses.exists():
            for bundled, original in [("mimo-jp-Mozc-BSD.txt", "MOZC-LICENSE.txt"), ("mimo-jp-Unicode-3.0.txt", "UNICODE-LICENSE.txt"),
                                      ("mimo-jp-SOURCE.json", "SOURCE.json"), ("mimo-jp-BSD.txt", "BSD.txt"),
                                      ("mimo-jp-derived-BSD.txt", "LICENSE.txt"), ("mimo-jp-COPYING.txt", "COPYING.txt")]:
                path = licenses / bundled
                check(path.exists() and path.read_bytes() == (ROOT / "DataSources/mimo-jp" / original).read_bytes(),
                      f"bundled notice {bundled} differs")
    for failure in failures[:30]:
        print("FAIL:", failure)
    print("FAIL" if failures else "PASS", f"({len(failures)} failed checks)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
