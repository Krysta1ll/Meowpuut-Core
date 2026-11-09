#!/usr/bin/env python3
"""Both traditional phrase spellings remain selectable; simplified output is deduplicated.

Usage: python3 Core/tests/check_traditional_phrase_variants.py [resource-directory]
"""
import ctypes
import importlib.util
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0, str(root / 'scripts'))
spec = importlib.util.spec_from_file_location('lexicon_compiler', root / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# The two traditional forms have independent upstream readings. They cannot
# be treated as duplicates simply because they share a simplified spelling.
cases = [
    (('yi1', 'zhou1'), ('一周', '一週'), '一周'),
    (('yi4', 'zhou1'), ('一周', '一週'), '一周'),
    (('ji4', 'lu4'), ('紀錄', '記錄'), '记录'),
]
with sqlite3.connect(resources / 'language-evidence.sqlite') as db:
    for sounds, traditional, simplified in cases:
        toned = ''.join(sounds)
        rows = db.execute('SELECT hant,hans FROM lexicon WHERE toned=? AND hans=?',
                          (toned, simplified)).fetchall()
        for form in traditional:
            assert (form, simplified) in rows, (toned, form, rows)

with tempfile.TemporaryDirectory(prefix='meowpuut-phrase-variants-') as tmp:
    library = Path(tmp) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags,
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ('mpn_lexicon_destroy', [ctypes.c_void_p], None),
        ('mpn_session_create', [ctypes.c_void_p], ctypes.c_void_p),
        ('mpn_session_destroy', [ctypes.c_void_p], None),
        ('mpn_set_simplified', [ctypes.c_void_p, ctypes.c_int], None),
        ('mpn_clear', [ctypes.c_void_p], None),
        ('mpn_key', [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ('mpn_candidate_count', [ctypes.c_void_p], ctypes.c_size_t),
        ('mpn_candidate', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
        ('mpn_choose', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_int),
        ('mpn_committed', [ctypes.c_void_p], ctypes.c_char_p),
    ]:
        function = getattr(core, name)
        function.argtypes, function.restype = args, result
    checks = 0
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon
        session = core.mpn_session_create(lexicon)
        assert session
        try:
            for sounds, traditional, simplified in cases:
                for use_tone in (False, True):
                    if mode == 'pinyin':
                        raw = ''.join(sounds) if use_tone else re.sub('[1-5]', '', ''.join(sounds))
                    else:
                        encoded = [compiler.encode(sound) for sound in sounds]
                        assert all(encoded), sounds
                        raw = ''.join(encoded) if use_tone else ''.join(value[:-1] for value in encoded)
                    for simplified_mode, expected in [(False, traditional), (True, (simplified,))]:
                        for word in expected:
                            core.mpn_clear(session)
                            core.mpn_set_simplified(session, simplified_mode)
                            for key in raw:
                                assert core.mpn_key(session, ord(key)), (mode, raw, key)
                            candidates = [core.mpn_candidate(session, index).decode('utf-8')
                                          for index in range(core.mpn_candidate_count(session))]
                            assert word in candidates, (mode, raw, simplified_mode, word, candidates[:25])
                            if simplified_mode:
                                assert candidates.count(word) == 1, (mode, raw, word)
                            assert core.mpn_choose(session, candidates.index(word))
                            assert core.mpn_committed(session).decode('utf-8') == word
                            checks += 1
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
print(f'PASS: {checks} phrase variant selections, both modes, scripts and tone inputs')
