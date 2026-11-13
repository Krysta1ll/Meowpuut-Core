#!/usr/bin/env python3
"""Compare the Chinese candidates of the Rust core with committed goldens.

  python3 scripts/check-chinese-golden.py                 compare (exit 0 same, 1 changed, 2 lexicon changed)
  python3 scripts/check-chinese-golden.py --write         write the goldens from the current build
  python3 scripts/check-chinese-golden.py --make-inputs [--parity DIR]
                                                          rewrite the input lists (rarely needed)

Options: --resources DIR  compiled lexicon directory (default .build/lexicon)
         --lib PATH       libmeowpuut_ffi to load; without it the library is built with cargo
         --golden DIR     where inputs and goldens live (default Tests/golden)

Every input is typed key by key into a fresh composition. After each key the
candidate count and the first five candidates are recorded, so every prefix of
every input has a row. Pinyin sessions have the mixed pool registered the way
the clients do it (first entry of each reading in native-japanese.json).

The golden header holds the sha256 of mimo_base.bin. When the lexicon was
changed on purpose the hash differs and the script says so (exit 2); the
goldens are then rewritten with --write. A difference with the same hash is a
regression (exit 1).
"""
import argparse
import ctypes
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOP = 5
MAX_INPUT = 256

# Da-Chien key map, the same table as KEYS and SYMBOLS in
# crates/meowpuut-core/src/reading.rs. The space bar is the first tone.
ZHUYIN_KEYS = "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/- 6347"
ZHUYIN_SYMBOLS = "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦˉˊˇˋ˙"
KEY_FOR_SYMBOL = dict(zip(ZHUYIN_SYMBOLS, ZHUYIN_KEYS))
SYMBOL_FOR_KEY = dict(zip(ZHUYIN_KEYS, ZHUYIN_SYMBOLS))

# Plain Chinese sentences used by the design measurements (42).
SENTENCES = """
womenjintianqukandianying jintiantianqihenhao nihaowoshizhongguoren zhegewentihennanjiejue
mingtianyiqiquchifanba xiexienidebangzhu wozaixuexiriyu nizuijinzenmeyang wojuedezhegebucuo
tamenyijingzoule qingwenxishoujianzainali wanshangyaobuyaoyiqichifan woshinanren tashinvren
womendoushihaopengyou zhejianshiqinghenzhongyao woxiangmaiyibenshu nikanguozhebudianyingma
wojintianyoudianmang zuotianwanshangwomeishuihao tadehanyushuodehenhao qingdengyixia
womenmingtianjian duibuqiwolaiwanle zhegeduoshaoqian wobuzhidaozenmeban nizaiganshenme
xianzaijidianle wohenxihuanzheshouge qingbangwokaiyixiamen shijieshanghaiyouhenduoren
womenyiqinuliba zhongwenhennandanshihenyouyisi tamendoushidaxuesheng woyaoyibeikafei
jintianshixingqiji womingtianyaoshangban nichifanlema wanan zaijian meiguanxi bukeqi
""".split()
HEAVIEST_PINYIN = 2000
HEAVIEST_ZHUYIN = 1000


def check_key_map():
    """The table above must be the one the core uses."""
    source = ROOT / "crates/meowpuut-core/src/reading.rs"
    if not source.exists():
        return
    text = source.read_text(encoding="utf-8")
    keys = re.search(r'pub const KEYS: &\[u8\] = b"([^"]+)";', text)
    symbols = re.search(r"pub const SYMBOLS: \[&str; \d+\] = \[(.*?)\];", text, re.S)
    if not keys or not symbols:
        sys.exit("check-chinese-golden: cannot read KEYS and SYMBOLS from reading.rs")
    names = re.findall(r'"([^"]*)"', symbols.group(1))
    core = dict(zip(names, keys.group(1)))
    for symbol, key in KEY_FOR_SYMBOL.items():
        if core.get(symbol) != key:
            sys.exit(f"check-chinese-golden: key map differs from reading.rs at {symbol}")


def lexicon_rows(path):
    """Rows of native-pinyin.tsv or native-zhuyin.tsv: bare, reading, hant, hans, weight."""
    with open(path, encoding="utf-8") as source:
        next(source)
        for line in source:
            if line.startswith("# context"):
                break
            fields = line.rstrip("\n").split("\t")
            if len(fields) == 5:
                yield fields


def heaviest(path, column, limit):
    best = {}
    for fields in lexicon_rows(path):
        key, weight = fields[column], int(fields[4])
        if weight > best.get(key, 0):
            best[key] = weight
    ranked = sorted(best.items(), key=lambda item: (-item[1], item[0]))
    return [key for key, _ in ranked[:limit]]


def make_inputs(resources, golden, parity):
    """Sentences and the heaviest readings in both scripts, parity rows in their own script."""
    pinyin, zhuyin = [], []
    design = SENTENCES + heaviest(resources / "native-pinyin.tsv", 0, HEAVIEST_PINYIN)
    for script in "ts":
        pinyin += [(script, keys) for keys in design]
    for keys in heaviest(resources / "native-zhuyin.tsv", 1, HEAVIEST_ZHUYIN):
        symbols = "".join(SYMBOL_FOR_KEY[key] for key in keys)
        zhuyin += [("t", symbols), ("s", symbols)]
    if parity:
        for name, target in (("all.pinyin-hant.jsonl", pinyin), ("all.pinyin-hans.jsonl", pinyin),
                             ("all.zhuyin.jsonl", zhuyin)):
            path = Path(parity) / name
            if not path.exists():
                print(f"skipped, not found: {path}")
                continue
            for line in path.read_text(encoding="utf-8").splitlines():
                row = json.loads(line)
                script = "s" if row["script"] == "hans" else "t"
                for field in ("keys", "alt_keys", "keys_apostrophe", "full_keys"):
                    value = row.get(field)
                    for keys in value if isinstance(value, list) else [value] if value else []:
                        target.append((script, keys))
    for name, rows, pattern in (("chinese-pinyin-inputs.txt", pinyin, r"[a-z']+"),
                                ("chinese-zhuyin-inputs.txt", zhuyin, f"[{ZHUYIN_SYMBOLS}]+")):
        seen, kept = set(), []
        for script, keys in rows:
            if (script, keys) in seen:
                continue
            seen.add((script, keys))
            if not re.fullmatch(pattern, keys) or len(keys) > MAX_INPUT:
                print(f"skipped, not typable: {keys!r}")
                continue
            kept.append(f"{script}\t{keys}")
        golden.mkdir(parents=True, exist_ok=True)
        head = ("# Inputs of scripts/check-chinese-golden.py: script (t Traditional, s Simplified), keys.\n"
                "# Written by --make-inputs. Zhuyin keys are symbols; ˉ is the first tone (space bar).\n")
        (golden / name).write_text(head + "\n".join(kept) + "\n", encoding="utf-8")
        print(f"{name}: {len(kept)} inputs")


def read_inputs(path):
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            script, keys = line.split("\t")
            rows.append((script, keys))
    return rows


def load_core(lib):
    if lib is None:
        env = {**os.environ, "PATH": f"{Path.home() / '.cargo/bin'}:{os.environ.get('PATH', '')}"}
        subprocess.run(["cargo", "build", "--release", "-p", "meowpuut-ffi"], cwd=ROOT, env=env, check=True)
        ext = "dylib" if sys.platform == "darwin" else ("dll" if sys.platform == "win32" else "so")
        prefix = "" if sys.platform == "win32" else "lib"
        lib = ROOT / "target/release" / f"{prefix}meowpuut_ffi.{ext}"
    core = ctypes.CDLL(str(lib))
    ptr, text, number, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    for name, result, arguments in (
            ("mpn_lexicon_create_from_base", ptr, (text, size, number)),
            ("mpn_lexicon_destroy", None, (ptr,)),
            ("mpn_session_create", ptr, (ptr,)),
            ("mpn_session_destroy", None, (ptr,)),
            ("mpn_set_simplified", None, (ptr, number)),
            ("mpn_add_japanese_word", number, (ptr, text, text, number)),
            ("mpn_key", number, (ptr, ctypes.c_int32)),
            ("mpn_clear", None, (ptr,)),
            ("mpn_committed", text, (ptr,)),
            ("mpn_candidate_count", size, (ptr,)),
            ("mpn_candidate", text, (ptr, size))):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
    return core


def candidates(core, base, resources, pinyin, inputs):
    """{(script, typed prefix): [count, candidate, ...]} for every prefix of every input."""
    lexicon = core.mpn_lexicon_create_from_base(base, len(base), 1 if pinyin else 0)
    if not lexicon:
        sys.exit("check-chinese-golden: mimo_base.bin did not load")
    pool = json.loads((resources / "native-japanese.json").read_text(encoding="utf-8")) if pinyin else {}
    sessions = {}
    for script in "ts":
        session = core.mpn_session_create(lexicon)
        core.mpn_set_simplified(session, 1 if script == "s" else 0)
        for reading, entries in pool.items():
            if entries:
                core.mpn_add_japanese_word(session, reading.encode(), entries[0]["text"].encode(), entries[0]["weight"])
        sessions[script] = session
    rows = {}
    for script, keys in inputs:
        session = sessions[script]
        core.mpn_clear(session)
        for end in range(1, len(keys) + 1):
            symbol = keys[end - 1]
            core.mpn_key(session, ord(symbol if pinyin else KEY_FOR_SYMBOL[symbol]))
            if core.mpn_committed(session):
                sys.exit(f"check-chinese-golden: input committed on its own: {keys}")
            if (script, keys[:end]) in rows:
                continue
            count = core.mpn_candidate_count(session)
            first = [core.mpn_candidate(session, index).decode("utf-8") for index in range(min(count, TOP))]
            if any("\t" in item or "\n" in item for item in first):
                sys.exit(f"check-chinese-golden: candidate with a tab or newline: {keys[:end]}")
            rows[(script, keys[:end])] = [str(count)] + first
    for session in sessions.values():
        core.mpn_session_destroy(session)
    core.mpn_lexicon_destroy(lexicon)
    return rows


def render(rows, digest, mode):
    lines = [f"# Chinese {mode} candidates of the core. Written by scripts/check-chinese-golden.py --write.",
             f"# mimo_base.bin sha256 {digest}",
             f"# script, typed keys, candidate count, first {TOP} candidates"]
    for (script, keys), values in sorted(rows.items()):
        lines.append("\t".join([script, keys] + values))
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--make-inputs", action="store_true")
    parser.add_argument("--parity")
    parser.add_argument("--resources", default=str(ROOT / ".build/lexicon"))
    parser.add_argument("--lib")
    parser.add_argument("--golden", default=str(ROOT / "Tests/golden"))
    arguments = parser.parse_args()
    resources, golden = Path(arguments.resources), Path(arguments.golden)
    check_key_map()
    if arguments.make_inputs:
        make_inputs(resources, golden, arguments.parity)
        return 0
    core = load_core(arguments.lib)
    base = (resources / "mimo_base.bin").read_bytes()
    digest = hashlib.sha256(base).hexdigest()
    status = 0
    for mode in ("pinyin", "zhuyin"):
        inputs = read_inputs(golden / f"chinese-{mode}-inputs.txt")
        rows = candidates(core, base, resources, mode == "pinyin", inputs)
        text = render(rows, digest, mode)
        path = golden / f"chinese-{mode}-top5.tsv"
        if arguments.write:
            path.write_text(text, encoding="utf-8")
            print(f"wrote {path.name}: {len(inputs)} inputs, {len(rows)} rows")
            continue
        if not path.exists():
            print(f"FAIL: {path} is missing; run with --write")
            status = max(status, 1)
            continue
        old = path.read_text(encoding="utf-8")
        if old == text:
            print(f"PASS: {path.name}: {len(inputs)} inputs, {len(rows)} rows")
            continue
        old_lines, new_lines = old.splitlines(), text.splitlines()
        lexicon_changed = old_lines[1:2] != new_lines[1:2]
        before = {tuple(line.split("\t")[:2]): line for line in old_lines if not line.startswith("#")}
        after = {tuple(line.split("\t")[:2]): line for line in new_lines if not line.startswith("#")}
        changed = sorted(key for key in before.keys() | after.keys() if before.get(key) != after.get(key))
        if lexicon_changed:
            print(f"LEXICON CHANGED: {path.name} was written for another mimo_base.bin "
                  f"({old_lines[1][2:] if len(old_lines) > 1 else 'no header'}; now sha256 {digest}); "
                  f"{len(changed)} rows differ. Rewrite with --write if the lexicon change was meant.")
            status = max(status, 2)
        else:
            print(f"FAIL: {path.name}: {len(changed)} of {len(after)} rows differ with the same lexicon")
            status = max(status, 1)
        for key in changed[:20]:
            print(f"  - {before.get(key, '(no row)')}")
            print(f"  + {after.get(key, '(no row)')}")
    return status


if __name__ == "__main__":
    sys.exit(main())
