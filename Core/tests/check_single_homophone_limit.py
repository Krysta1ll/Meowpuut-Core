#!/usr/bin/env python3
"""Verify bounded single-syllable homophone pagination and selection."""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='meowpuut-single-') as tmp:
    library = Path(tmp) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags, str(ROOT / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ('mpn_lexicon_destroy', [ctypes.c_void_p], None),
        ('mpn_session_create', [ctypes.c_void_p], ctypes.c_void_p),
        ('mpn_session_destroy', [ctypes.c_void_p], None),
        ('mpn_clear', [ctypes.c_void_p], None),
        ('mpn_set_simplified', [ctypes.c_void_p, ctypes.c_int], None),
        ('mpn_require_full_candidate', [ctypes.c_void_p], None),
        ('mpn_key', [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ('mpn_candidate_count', [ctypes.c_void_p], ctypes.c_size_t),
        ('mpn_candidate', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
        ('mpn_choose', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_int),
        ('mpn_page', [ctypes.c_void_p], ctypes.c_size_t),
        ('mpn_committed', [ctypes.c_void_p], ctypes.c_char_p),
        ('mpn_input', [ctypes.c_void_p], ctypes.c_char_p),
    ]:
        f = getattr(core, name); f.argtypes = args; f.restype = result

    for mode, readings in [('pinyin', [('yi', None), ('yi1', 0), ('yi4', 1)]),
                           ('zhuyin', [('u', None), ('u ', 0), ('u4', 1)])]:
        rows = []
        for i in range(600):
            bare = 'yi' if mode == 'pinyin' else 'u'
            reading = ('yi1', 'yi4')[i % 2] if mode == 'pinyin' else ('u ', 'u4')[i % 2]
            rows.append(f'{bare}\t{reading}\t{chr(0x4e00+i)}\t{chr(0x6000+i)}\t{10000-i}')
        data = (f'MEOWPUUT-{mode.upper()}-1\n' + '\n'.join(rows) + '\n').encode()
        owner = core.mpn_lexicon_create(data, len(data)); assert owner
        session = core.mpn_session_create(owner); assert session
        try:
            for simplified in (False, True):
                core.mpn_set_simplified(session, int(simplified))
                for android in ((False, True) if mode == 'pinyin' else (False,)):
                    if android: core.mpn_require_full_candidate(session)
                    for reading, tone in readings:
                        core.mpn_clear(session)
                        for key in reading: assert core.mpn_key(session, ord(key)), (mode, reading, key)
                        items = [core.mpn_candidate(session, i).decode() for i in range(core.mpn_candidate_count(session))]
                        expected = [chr((0x6000 if simplified else 0x4e00) + i)
                                    for i in range(600) if tone is None or i % 2 == tone]
                        assert all(char in items for char in expected), (mode, reading, simplified, len(items))
                        indices = [items.index(char) for char in expected]
                        assert indices == sorted(indices), (mode, reading, indices[:10])
                        target = expected[-1]; index = items.index(target)
                        assert index >= 256, (mode, reading, index)
                        for _ in range(index // 7): assert core.mpn_key(session, 0xff56)
                        assert core.mpn_page(session) == index // 7
                        assert core.mpn_choose(session, index)
                        assert core.mpn_committed(session).decode() == target
                        assert core.mpn_input(session) == b''
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(owner)

    # Multiple tones for the same written word must not consume the limited
    # sentence beam and hide another distinct character in a composed phrase.
    for mode, wo, xi, pending in [('pinyin', 'wo3', ('xi5', 'xi1', 'xi4', 'xi2', 'xi4', 'xi3'), 'woxi'),
                                  ('zhuyin', 'ji3', ('vu7', 'vu ', 'vu4', 'vu6', 'vu4', 'vu3'), 'jivu')]:
        entries = [('西', '西', 7297), ('西', '西', 6871), ('係', '系', 6749),
                   ('息', '息', 6632), ('系', '系', 6461), ('喜', '喜', 6235)]
        first = 'wo' if mode == 'pinyin' else 'ji'
        second = 'xi' if mode == 'pinyin' else 'vu'
        rows = [f'{first}\t{wo}\t我\t我\t10000']
        rows.extend(f'{second}\t{tone}\t{hant}\t{hans}\t{weight}'
                    for tone, (hant, hans, weight) in zip(xi, entries))
        rows.append('huan\thuan1\t歡\t欢\t8000' if mode == 'pinyin'
                    else 'cj0\tcj0 \t歡\t欢\t8000')
        rows.sort(key=lambda row: (row.split('\t')[0], -int(row.split('\t')[-1])))
        data = (f'MEOWPUUT-{mode.upper()}-1\n' + '\n'.join(rows) + '\n').encode()
        owner = core.mpn_lexicon_create(data, len(data)); assert owner
        session = core.mpn_session_create(owner); assert session
        try:
            for simplified in (False, True):
                core.mpn_clear(session)
                core.mpn_set_simplified(session, int(simplified))
                for key in pending: assert core.mpn_key(session, ord(key))
                candidates = [core.mpn_candidate(session, i).decode()
                              for i in range(core.mpn_candidate_count(session))]
                assert '我喜' in candidates[:7], (mode, simplified, candidates[:7])
                assert core.mpn_choose(session, candidates.index('我喜'))
                assert core.mpn_committed(session).decode() == '我喜'
                assert core.mpn_input(session) == b''
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(owner)

    # A malformed or unusually large bucket remains bounded; full sentence
    # lookup preserves the old, lower limit and does not grow with the lexicon.
    for syllable, count, length, expected in [('yi', 256, 1, 256), ('yi', 2100, 1, 2048), ('yiyi', 400, 2, 256)]:
        rows = []
        for i in range(count):
            text = chr(0x4e00 + i) * length
            rows.append(f'{syllable}\t{"yi1" * length}\t{text}\t{text}\t{5000-i}')
        data = ('MEOWPUUT-PINYIN-1\n' + '\n'.join(rows) + '\n').encode()
        owner = core.mpn_lexicon_create(data, len(data)); assert owner
        session = core.mpn_session_create(owner); assert session
        try:
            for key in syllable: assert core.mpn_key(session, ord(key))
            assert core.mpn_candidate_count(session) == expected, (syllable, core.mpn_candidate_count(session))
            if count == 256:
                core.mpn_require_full_candidate(session)
                assert core.mpn_candidate_count(session) == 257
                assert chr(0x4e00 + 255) in [core.mpn_candidate(session, i).decode()
                                              for i in range(core.mpn_candidate_count(session))]
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(owner)
print('PASS: single-syllable coverage, modes/scripts/tones, pagination, Android, bounded sentences')
