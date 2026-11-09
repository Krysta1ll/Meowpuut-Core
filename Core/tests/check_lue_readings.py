#!/usr/bin/env python3
"""Check lüe/nüe normalization and aliases against a compiled mimo lexicon."""
import ctypes
import importlib.util
from pathlib import Path
import re
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / ".build/Meowpuut.app/Contents/Resources"
sys.path.insert(0, str(root / "scripts"))
spec = importlib.util.spec_from_file_location("mimo_compiler", root / "scripts/compile-native-lexicon.py")
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

with tempfile.TemporaryDirectory(prefix="mimo-lue-") as temporary:
    library = Path(temporary) / ("core.dylib" if sys.platform == "darwin" else "core.so")
    flags = ["-dynamiclib"] if sys.platform == "darwin" else ["-shared", "-fPIC"]
    subprocess.run(["c++", "-std=c++17", "-O2", *flags, str(root / "Core/MeowpuutCore.cpp"), "-o", str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ("mpn_lexicon_create", [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ("mpn_lexicon_destroy", [ctypes.c_void_p], None),
        ("mpn_session_create", [ctypes.c_void_p], ctypes.c_void_p),
        ("mpn_session_destroy", [ctypes.c_void_p], None),
        ("mpn_set_simplified", [ctypes.c_void_p, ctypes.c_int], None),
        ("mpn_clear", [ctypes.c_void_p], None),
        ("mpn_key", [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ("mpn_candidate_count", [ctypes.c_void_p], ctypes.c_size_t),
        ("mpn_candidate", [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
        ("mpn_choose", [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_int),
        ("mpn_committed", [ctypes.c_void_p], ctypes.c_char_p),
    ]:
        function = getattr(core, name)
        function.argtypes, function.restype = args, result

    cases = [("lve4", ("略", "略")), ("lve4", ("掠", "掠")),
             ("nve4", ("虐", "虐")), ("nve4", ("瘧", "疟")),
             ("sheng3lve4", ("省略", "省略"))]
    for mode in ("pinyin", "zhuyin"):
        data = (resources / f"native-{mode}.tsv").read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, mode
        session = core.mpn_session_create(lexicon)
        assert session
        try:
            for reading, variants in cases:
                # Tone-free and tone-qualified input; pinyin accepts both v and u aliases.
                spellings = [reading] if mode == "zhuyin" else [reading, reading.replace("ve", "ue")]
                for spelling in spellings:
                    if mode == "zhuyin":
                        parts = spelling.replace("sheng3", "sheng3 ").split()
                        raw = "".join(compiler.encode(part) for part in parts)
                    else:
                        raw = spelling
                    plain = re.sub("[1-5]", "", raw) if mode == "pinyin" else re.sub("[ 6347]", "", raw)
                    for keys in (raw, plain):
                        for simplified in (0, 1):
                            core.mpn_clear(session)
                            core.mpn_set_simplified(session, simplified)
                            for key in keys:
                                assert core.mpn_key(session, ord(key)), (mode, keys, key)
                            candidates = [core.mpn_candidate(session, i).decode() for i in range(core.mpn_candidate_count(session))]
                            expected = variants[simplified]
                            assert expected in candidates, (mode, keys, expected, candidates[:20])
                            assert core.mpn_choose(session, candidates.index(expected))
                            assert core.mpn_committed(session).decode() == expected
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
print("PASS: lüe/nüe aliases, Zhuyin, phrase, scripts and selection")
