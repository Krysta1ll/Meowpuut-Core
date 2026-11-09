#!/usr/bin/env python3
"""Check reviewed ni3 pronoun conversion and both input modes."""
import ctypes
import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0, str(root / 'scripts'))
spec = importlib.util.spec_from_file_location('mimo_compiler', root / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# Existing correct simplified source keeps its own weight; the traditional
# spelling and native-key membership survive. Unaligned and nai3 are untouched.
old = ('妳的', '妳的', 'ni3 de5')
good = ('妳的', '你的', 'ni3 de5')
unmapped = ('妳', '妳', 'nai3')
unaligned = ('名字', '妳名字', 'ni3 ming2 zi4')
words = {old: 9000, good: 7000, unmapped: 3000, unaligned: 1000}
native = {old, unmapped, unaligned}
changes = compiler.normalize_simplified_pronouns(words, native)
assert words == {good: 7000, unmapped: 3000, unaligned: 1000}
assert native == {good, unmapped, unaligned}
assert len(changes) == 1 and changes[0]['retained_existing_weight'] == 7000
words = {old: 9000, good: 7000}
native = {old}
try:
    compiler.normalize_simplified_pronouns(words, native, {old})
    raise AssertionError('protected reviewed entry must fail')
except ValueError:
    assert words == {old: 9000, good: 7000} and native == {old}

with sqlite3.connect(resources / 'language-evidence.sqlite') as db:
    for hant, hans, toned in [('妳的', '你的', 'ni3de5'), ('妳們', '你们', 'ni3men5'),
                              ('謝謝妳', '谢谢你', 'xie4xie5ni3'), ('妳好嗎', '你好吗', 'ni3hao3ma1')]:
        assert db.execute('SELECT 1 FROM lexicon WHERE hant=? AND hans=? AND toned=?',
                          (hant, hans, toned)).fetchone(), (hant, hans, toned)
        assert not db.execute('SELECT 1 FROM lexicon WHERE hant=? AND hans LIKE ? AND toned=?',
                              (hant, '%妳%', toned)).fetchone(), (hant, toned)
    assert db.execute("SELECT 1 FROM lexicon WHERE hant='妳' AND hans='妳' AND toned='nai3'").fetchone()
report = json.loads((resources / 'simplified-pronoun-audit.json').read_text(encoding='utf-8'))
assert any(row['traditional'] == '妳的' and row['simplified'] == '你的' for row in report)

with tempfile.TemporaryDirectory(prefix='meowpuut-pronoun-test-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags, str(root / 'Core/MeowpuutCore.cpp'),
                    '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    ptr, size = ctypes.c_void_p, ctypes.c_size_t
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, size], ptr), ('mpn_lexicon_destroy', [ptr], None),
        ('mpn_session_create', [ptr], ptr), ('mpn_session_destroy', [ptr], None),
        ('mpn_set_simplified', [ptr, ctypes.c_int], None), ('mpn_clear', [ptr], None),
        ('mpn_key', [ptr, ctypes.c_int32], ctypes.c_int), ('mpn_candidate_count', [ptr], size),
        ('mpn_candidate', [ptr, size], ctypes.c_char_p), ('mpn_choose', [ptr, size], ctypes.c_int),
        ('mpn_committed', [ptr], ctypes.c_char_p),
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result
    cases = [('ni3 de5', '妳的', '你的'), ('ni3 men5', '妳們', '你们'),
             ('xie4 xie5 ni3', '謝謝妳', '谢谢你'), ('ni3 hao3 ma1', '妳好嗎', '你好吗')]
    checks = 0
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data)); assert lexicon
        session = core.mpn_session_create(lexicon); assert session
        try:
            for reading, traditional, simplified in cases:
                sounds = reading.split()
                keys = ''.join(sounds) if mode == 'pinyin' else ''.join(compiler.encode(s) for s in sounds)
                for script, target in [(0, traditional), (1, simplified)]:
                    core.mpn_clear(session); core.mpn_set_simplified(session, script)
                    for key in keys: assert core.mpn_key(session, ord(key)), (mode, keys, key)
                    candidates = [core.mpn_candidate(session, index).decode()
                                  for index in range(core.mpn_candidate_count(session))]
                    assert target in candidates, (mode, keys, script, target, candidates[:20])
                    if script: assert all('妳' not in text for text in candidates), (mode, keys, candidates[:20])
                    assert core.mpn_choose(session, candidates.index(target))
                    assert core.mpn_committed(session).decode() == target
                    checks += 1
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)
print(f'PASS: safe simplified pronouns, existing source weights, nai3 and {checks} selections')
