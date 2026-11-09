#!/usr/bin/env python3
"""Check confirmed traditional-source aliases against compiled resources.

Usage: python3 Core/tests/check_simplified_source_variants.py [resource-directory]
The SQLite and native tables must have been generated from the same sources.
"""
import ctypes
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / ".build/Meowpuut.app/Contents/Resources"
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("mimo_lexicon_compiler", ROOT / "scripts/compile-native-lexicon.py")
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# Both data sources independently contain each character with the same
# reading. A duplicate traditional PinyinPro key must not become simplified
# output or replace the actual simplified source's weight.
confirmed = [
    ("jian3", "簡", "简"),
    ("duan4", "鍛", "锻"),
    ("bei4", "備", "备"),
    ("mie4", "滅", "灭"),
]
# These are valid simplified forms, or have an ambiguous conversion/reading.
# Automatically converting them based only on a transform would lose meaning.
ambiguous = [("ju4", "俱"), ("jie4", "藉"), ("zhu4", "著"), ("qian2", "乾")]

source = json.loads((ROOT / "DataSources/PinyinPro/complete.json").read_text(encoding="utf-8"))
with sqlite3.connect(resources / "language-evidence.sqlite") as db:
    for reading, traditional, simplified in confirmed:
        rows = db.execute("SELECT hant,hans,weight FROM lexicon WHERE toned=? AND hant=?",
                          (reading, traditional)).fetchall()
        expected_weight = max(1, round(700 * math.log1p(float(source[simplified][1]) * 1e12)))
        assert (traditional, simplified, expected_weight) in rows, (reading, rows, expected_weight)
        assert all(hans != traditional for _, hans, _ in rows), (reading, rows)
    for reading, word in ambiguous:
        rows = db.execute("SELECT hant,hans FROM lexicon WHERE toned=? AND hans=?",
                          (reading, word)).fetchall()
        assert rows, (reading, word)

with tempfile.TemporaryDirectory(prefix="mimo-source-variants-") as temporary:
    library = Path(temporary) / ("core.dylib" if sys.platform == "darwin" else "core.so")
    flags = ["-dynamiclib"] if sys.platform == "darwin" else ["-shared", "-fPIC"]
    subprocess.run([os.environ.get("CXX", "c++"), "-std=c++17", "-O2", *flags,
                    str(ROOT / "Core/MeowpuutCore.cpp"), "-o", str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ("mpn_lexicon_create", [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ("mpn_lexicon_destroy", [ctypes.c_void_p], None),
        ("mpn_session_create", [ctypes.c_void_p], ctypes.c_void_p),
        ("mpn_session_destroy", [ctypes.c_void_p], None),
        ("mpn_clear", [ctypes.c_void_p], None),
        ("mpn_set_simplified", [ctypes.c_void_p, ctypes.c_int], None),
        ("mpn_key", [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ("mpn_candidate_count", [ctypes.c_void_p], ctypes.c_size_t),
        ("mpn_candidate", [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
        ("mpn_choose", [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_int),
        ("mpn_committed", [ctypes.c_void_p], ctypes.c_char_p),
    ]:
        function = getattr(core, name)
        function.argtypes, function.restype = args, result

    count = 0
    for mode in ("pinyin", "zhuyin"):
        data = (resources / f"native-{mode}.tsv").read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, mode
        session = core.mpn_session_create(lexicon)
        assert session, mode
        try:
            def get_candidates(reading, simplified, toned):
                core.mpn_clear(session)
                core.mpn_set_simplified(session, simplified)
                keys = reading if mode == "pinyin" else compiler.encode(reading)
                if not toned:
                    keys = re.sub("[1-5]", "", keys) if mode == "pinyin" else keys[:-1]
                for key in keys:
                    assert core.mpn_key(session, ord(key)), (mode, keys)
                return [core.mpn_candidate(session, i).decode("utf-8")
                        for i in range(core.mpn_candidate_count(session))]

            for reading, traditional, simplified in confirmed:
                for toned in (False, True):
                    for use_simplified in (False, True):
                        candidates = get_candidates(reading, use_simplified, toned)
                        expected = simplified if use_simplified else traditional
                        assert expected in candidates, (mode, reading, toned, expected, candidates[:20])
                        if use_simplified:
                            assert traditional not in candidates, (mode, reading, toned, candidates[:20])
                            assert candidates.count(simplified) == 1, (mode, reading, toned)
                        assert core.mpn_choose(session, candidates.index(expected))
                        assert core.mpn_committed(session).decode("utf-8") == expected
                        count += 1
            for reading, word in ambiguous:
                candidates = get_candidates(reading, True, True)
                assert word in candidates, (mode, reading, word, candidates[:20])
                assert core.mpn_choose(session, candidates.index(word))
                assert core.mpn_committed(session).decode("utf-8") == word
                count += 1
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

print(f"PASS: {count} source-variant candidate selections, both modes and scripts; ambiguous controls retained")
