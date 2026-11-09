#!/usr/bin/env python3
"""Exact single-syllable characters precede nasal corrections in both scripts."""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'

class Info(ctypes.Structure):
    _fields_ = [('kind', ctypes.c_uint32), ('score_kind', ctypes.c_uint32),
                ('consumed', ctypes.c_uint64), ('remaining', ctypes.c_uint64),
                ('score', ctypes.c_double)]

with tempfile.TemporaryDirectory(prefix='meowpuut-nasal-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags,
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    ptr, size = ctypes.c_void_p, ctypes.c_size_t
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, size], ptr),
        ('mpn_lexicon_destroy', [ptr], None), ('mpn_session_create', [ptr], ptr),
        ('mpn_session_destroy', [ptr], None), ('mpn_set_simplified', [ptr, ctypes.c_int], None),
        ('mpn_clear', [ptr], None), ('mpn_key', [ptr, ctypes.c_int32], ctypes.c_int),
        ('mpn_candidate_count', [ptr], size), ('mpn_candidate', [ptr, size], ctypes.c_char_p),
        ('mpn_candidate_info', [ptr, size, ctypes.POINTER(Info), size], ctypes.c_int),
    ]:
        function = getattr(core, name)
        function.argtypes, function.restype = args, result
    data = (resources / 'native-pinyin.tsv').read_bytes()
    lexicon = core.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = core.mpn_session_create(lexicon)
    assert session
    try:
        for simplified in (0, 1):
            core.mpn_set_simplified(session, simplified)
            for reading in ('bang', 'tan', 'tang', 'zhen', 'zeng', 'fan', 'feng'):
                core.mpn_clear(session)
                for char in reading:
                    assert core.mpn_key(session, ord(char))
                count = core.mpn_candidate_count(session)
                assert count >= 7, (reading, count)
                info = []
                for index in range(count):
                    item = Info()
                    assert core.mpn_candidate_info(session, index, ctypes.byref(item), ctypes.sizeof(item))
                    info.append(item.kind)
                assert 4 not in info[:7], (reading, simplified, info[:7])
                if reading == 'bang':
                    assert 4 in info[7:], (reading, simplified, 'correction unavailable')
            core.mpn_clear(session)
            for char in 'bang':
                assert core.mpn_key(session, ord(char))
            first = [core.mpn_candidate(session, index).decode() for index in range(7)]
            assert ('办' if simplified else '辦') not in first, first
            assert ('榜' if simplified else '榜') in first, first
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)
print('PASS: exact single-syllable candidates precede reachable nasal corrections in both scripts')
