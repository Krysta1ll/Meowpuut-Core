#!/usr/bin/env python3
"""Check everyday single-character ranking against compiled mimo resources.

Usage: python3 scripts/check-common-characters.py <compiled-resource-directory>
Requires a C++17 compiler. No network access or user history is used.
"""
import ctypes
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / '.build/lexicon'
spec = importlib.util.spec_from_file_location('lexicon_compiler', ROOT / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# The first two are contextual homophones in both scripts; neither should be
# displaced by the less common character when there is no learned preference.
cases = [
    ('bang1', ('幫', '帮'), ('浜', '浜')),
    ('hang2', ('行', '行'), ('絎', '绗')),
    ('ren4', ('認', '认'), ('紝', '紝')),
    ('qing2', ('情', '情'), ('殑', '殑')),
    ('hao4', ('號', '号'), ('灝', '灏')),
]
if True:
    env = {**os.environ, 'PATH': f"{Path.home() / '.cargo/bin'}:{os.environ.get('PATH', '')}"}
    subprocess.run(['cargo', 'build', '--release', '-p', 'meowpuut-ffi'], cwd=ROOT, env=env, check=True)
    ext = 'dylib' if sys.platform == 'darwin' else ('dll' if sys.platform == 'win32' else 'so')
    prefix_name = '' if sys.platform == 'win32' else 'lib'
    library = ROOT / 'target/release' / f'{prefix_name}meowpuut_ffi.{ext}'
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
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result
    checks = 0
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        if not lexicon: raise ValueError(f'invalid {mode} lexicon')
        session = core.mpn_session_create(lexicon)
        if not session: raise MemoryError('cannot create session')
        try:
            for simplified in (0, 1):
                core.mpn_set_simplified(session, simplified)
                for reading, common, rare in cases:
                    for with_tone in (False, True):
                        core.mpn_clear(session)
                        if mode == 'pinyin':
                            keys = reading if with_tone else re.sub('[1-5]', '', reading)
                        else:
                            keys = compiler.encode(reading)
                            if not with_tone: keys = keys[:-1]
                        for key in keys:
                            assert core.mpn_key(session, ord(key)), (mode, reading, keys)
                        result = [core.mpn_candidate(session, i).decode('utf-8')
                                  for i in range(core.mpn_candidate_count(session))]
                        expected, less_common = common[simplified], rare[simplified]
                        assert expected in result[:7], (mode, keys, expected, result[:7])
                        assert less_common not in result or result.index(expected) < result.index(less_common), (
                            mode, keys, expected, less_common, result[:15])
                        if reading == 'bang1':
                            assert result[0] == expected, (mode, keys, result[:7])
                            if not with_tone:
                                for char in (('棒', '棒'), ('綁', '绑'), ('榜', '榜')):
                                    assert char[simplified] in result[:7], (mode, keys, result[:7])
                        checks += 1
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
print(f'PASS: {checks} common-character checks across modes, scripts and tone inputs')
