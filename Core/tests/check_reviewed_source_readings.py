#!/usr/bin/env python3
"""Check the reviewed McBopomofo reading exclusion with compiled resources."""
import ctypes
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / ".build/Meowpuut.app/Contents/Resources"
audit = json.loads((resources / "source-reading-audit.json").read_text(encoding="utf-8"))
reviewed = audit["reviewed_exclusions"]
assert reviewed["missing_rule_ids"] == []
assert [(row["id"], row["word"], row["pinyin"]) for row in reviewed["matched"]] == [
    ("mcbopomofo-yi-ye4-conflict", "意", "ye4")
]
with sqlite3.connect(resources / "language-evidence.sqlite") as db:
    assert db.execute("SELECT 1 FROM lexicon WHERE hant='意' AND toned='yi4'").fetchone()
    assert not db.execute("SELECT 1 FROM lexicon WHERE hant='意' AND toned='ye4'").fetchone()

with tempfile.TemporaryDirectory(prefix="mimo-reviewed-reading-") as temporary:
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
    for mode, yes, no in (("pinyin", "yi4", "ye4"), ("zhuyin", "u4", "u,4")):
        data = (resources / f"native-{mode}.tsv").read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon
        session = core.mpn_session_create(lexicon)
        assert session
        try:
            for simplified in (0, 1):
                for keys, expected in ((yes, True), (no, False)):
                    core.mpn_clear(session)
                    core.mpn_set_simplified(session, simplified)
                    for key in keys:
                        assert core.mpn_key(session, ord(key)), (mode, keys)
                    candidates = [core.mpn_candidate(session, i).decode() for i in range(core.mpn_candidate_count(session))]
                    if expected:
                        assert "意" in candidates[:7], (mode, keys, candidates[:7])
                        assert core.mpn_choose(session, candidates.index("意"))
                        assert core.mpn_committed(session).decode() == "意"
                    else:
                        assert "意" not in candidates[:7], (mode, keys, candidates[:7])
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
print("PASS: reviewed source exclusion preserves yi4 and keeps ye4 first page clean")
