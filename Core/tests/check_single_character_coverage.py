#!/usr/bin/env python3
"""Verify standalone traditional variants and simplified dedup against compiled resources.

Usage: python3 Core/tests/check_single_character_coverage.py [resource-directory]
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

# Each pair consists of distinct traditional characters sharing one simplified
# spelling and pronunciation. The character must not only exist inside a word.
cases = [
    ('zhi4', ('志', '誌'), '志'),
    ('zhou1', ('周', '週'), '周'),
    ('xi4', ('系', '係'), '系'),
    ('yu4', ('愈', '癒'), '愈'),
    ('fu4', ('復', '複'), '复'),
    ('ju2', ('局', '侷'), '局'),
    ('nian4', ('念', '唸'), '念'),
    # Explicit low-frequency source characters also remain independently usable.
    ('de1', ('嘚',), '嘚'),
    ('tong2', ('砼',), '砼'),
]
with sqlite3.connect(resources / 'language-evidence.sqlite') as db:
    for reading, traditional, simplified in cases:
        rows = db.execute('SELECT hant, hans FROM lexicon WHERE reading=? AND toned=?',
                          (reading[:-1], reading)).fetchall()
        for char in traditional:
            assert (char, simplified) in rows, (reading, char, rows[:20])
    # A phrase-level conversion can use different words in the two scripts.
    # Its aligned characters must not invent an independent 紀→记 mapping.
    for hant, hans in [('紀', '纪'), ('記', '记'), ('蹟', '迹')]:
        assert db.execute('SELECT 1 FROM lexicon WHERE hant=? AND hans=? AND toned=?',
                          (hant, hans, 'ji4')).fetchone(), (hant, hans)
    assert not db.execute('SELECT 1 FROM lexicon WHERE hant=? AND hans=? AND toned=?',
                          ('紀', '记', 'ji4')).fetchone(), 'phrase alignment invented 紀→记'
    assert db.execute('SELECT 1 FROM lexicon WHERE hant=? AND hans=? AND toned=?',
                      ('紀錄', '记录', 'ji4lu4')).fetchone(), 'reviewed phrase must remain'

with tempfile.TemporaryDirectory(prefix='meowpuut-single-coverage-') as tmp:
    library = Path(tmp) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags, str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
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
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result
    checks = 0
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, mode
        session = core.mpn_session_create(lexicon)
        assert session, mode
        try:
            for reading, traditional, simplified in cases:
                for toned in (False, True):
                    raw = reading if mode == 'pinyin' else compiler.encode(reading)
                    if not toned:
                        raw = re.sub('[1-5]', '', raw) if mode == 'pinyin' else raw[:-1]
                    for simplified_mode in (False, True):
                        expected = (simplified,) if simplified_mode else traditional
                        for char in expected:
                            core.mpn_clear(session)
                            core.mpn_set_simplified(session, simplified_mode)
                            for key in raw:
                                assert core.mpn_key(session, ord(key)), (mode, raw, key)
                            candidates = [core.mpn_candidate(session, i).decode('utf-8')
                                          for i in range(core.mpn_candidate_count(session))]
                            assert char in candidates, (mode, raw, simplified_mode, char, candidates[:25])
                            if simplified_mode:
                                assert candidates.count(char) == 1, (mode, raw, char)
                                assert all(variant not in candidates for variant in traditional if variant != simplified), (
                                    mode, raw, traditional, candidates[:25])
                            assert core.mpn_choose(session, candidates.index(char))
                            assert core.mpn_committed(session).decode('utf-8') == char
                            checks += 1
            for toned in (False, True):
                raw = 'ji4' if mode == 'pinyin' else compiler.encode('ji4')
                if not toned:
                    raw = re.sub('[1-5]', '', raw) if mode == 'pinyin' else raw[:-1]
                for simplified_mode, expected in [(False, ('紀', '記', '蹟')), (True, ('纪', '记', '迹'))]:
                    for char in expected:
                        core.mpn_clear(session)
                        core.mpn_set_simplified(session, simplified_mode)
                        for key in raw: assert core.mpn_key(session, ord(key)), (mode, raw, key)
                        candidates = [core.mpn_candidate(session, i).decode('utf-8')
                                      for i in range(core.mpn_candidate_count(session))]
                        assert char in candidates, (mode, raw, simplified_mode, char, candidates[:25])
                        assert core.mpn_choose(session, candidates.index(char))
                        assert core.mpn_committed(session).decode('utf-8') == char
                        checks += 1
            for simplified_mode, expected in [(False, '紀錄'), (True, '记录')]:
                raw = 'ji4lu4' if mode == 'pinyin' else compiler.encode('ji4') + compiler.encode('lu4')
                core.mpn_clear(session)
                core.mpn_set_simplified(session, simplified_mode)
                for key in raw: assert core.mpn_key(session, ord(key)), (mode, raw, key)
                candidates = [core.mpn_candidate(session, i).decode('utf-8')
                              for i in range(core.mpn_candidate_count(session))]
                assert expected in candidates, (mode, raw, simplified_mode, expected, candidates[:25])
                assert core.mpn_choose(session, candidates.index(expected))
                assert core.mpn_committed(session).decode('utf-8') == expected
                checks += 1
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
print(f'PASS: {checks} standalone variant selections, both scripts and input modes')
