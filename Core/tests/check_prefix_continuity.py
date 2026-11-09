#!/usr/bin/env python3
"""Completed prefix homophones remain selectable while typing the next word.

This is a curated regression, not an unseen accuracy evaluation. Candidate
availability is required, not a forced first-place ranking or historical cache.
MEOWPUUT_PREFIX_CORE can point to an isolated experimental core source.
"""
import ctypes
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
RESOURCES = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / '.build/Meowpuut.app/Contents/Resources'
SOURCE = Path(os.environ.get('MEOWPUUT_PREFIX_CORE', ROOT / 'Core/MeowpuutCore.cpp'))
CASES = [
    ('zhuyin', 'ji3183ul4g7', 'z', ('我把鑰匙', '我把钥匙')),
    ('zhuyin', '5k41p3gj dk3u3ru,4', 'e', ('這本書可以借', '这本书可以借')),
    ('zhuyin', '1; ji31835k4ek72u453ru4', 'v', ('幫我把這個地址記', '帮我把这个地址记')),
    ('pinyin', 'maozaishafaxiamian', 'shu', ('貓在沙發下面', '猫在沙发下面')),
    ('pinyin', 'bagangcaidewenjian', 'ba', ('把剛才的文件', '把刚才的文件')),
    ('pinyin', 'daojiayihougeiwofa', 'xiao', ('到家以後給我發', '到家以后给我发')),
]

with tempfile.TemporaryDirectory(prefix='meowpuut-continuity-') as directory:
    library = Path(directory) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    shared = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *shared, str(SOURCE), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    P, S, N, Z = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    for name, result, args in [
        ('mpn_lexicon_create', P, [S, Z]), ('mpn_lexicon_destroy', None, [P]),
        ('mpn_session_create', P, [P]), ('mpn_session_destroy', None, [P]),
        ('mpn_key', N, [P, ctypes.c_int32]), ('mpn_clear', None, [P]),
        ('mpn_set_simplified', None, [P, N]), ('mpn_candidate_count', Z, [P]),
        ('mpn_candidate', S, [P, Z]), ('mpn_candidate_consumed', Z, [P, Z]),
        ('mpn_choose', N, [P, Z]), ('mpn_input', S, [P]), ('mpn_committed', S, [P]),
    ]:
        function = getattr(core, name)
        function.restype, function.argtypes = result, args

    def candidates(session):
        return [(core.mpn_candidate(session, i).decode(), core.mpn_candidate_consumed(session, i))
                for i in range(core.mpn_candidate_count(session))]

    def enter(session, raw):
        for key in raw.encode():
            assert core.mpn_key(session, key), (raw, key)

    failures = []
    availability = []
    for mode in ('pinyin', 'zhuyin'):
        data = (RESOURCES / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon
        try:
            for _, prefix, suffix, targets in [row for row in CASES if row[0] == mode]:
                for simplified, target in enumerate(targets):
                    session = core.mpn_session_create(lexicon)
                    assert session
                    try:
                        core.mpn_set_simplified(session, simplified)
                        enter(session, prefix)
                        expected = (target, len(prefix))
                        assert expected in candidates(session), ('fixture prefix unavailable', mode, prefix, target)
                        for key in suffix:
                            enter(session, key)
                            current = candidates(session)
                            if expected not in current:
                                failures.append((mode, simplified, core.mpn_input(session).decode(), expected, current[:12]))
                        current = candidates(session)
                        assert len(current) <= 256, ('candidate count unbounded', len(current))
                        if expected in current:
                            index = current.index(expected)
                            availability.append((mode, simplified, index, len(current)))
                            # Editing and re-entering the last key must produce
                            # the same list, rather than sticky historical text.
                            assert core.mpn_key(session, 0xff08)
                            enter(session, suffix[-1])
                            assert candidates(session) == current, ('history-dependent ordering', mode, prefix, suffix)
                            assert core.mpn_choose(session, current.index(expected))
                            assert core.mpn_committed(session).decode() == target
                            assert core.mpn_input(session).decode() == suffix, ('wrong suffix', mode, prefix, suffix)
                        # A fresh unrelated reading must not inherit stale prefixes.
                        core.mpn_clear(session)
                        enter(session, 'ni' if mode == 'pinyin' else 'su3')
                        assert all(text != target for text, _ in candidates(session))
                    finally:
                        core.mpn_session_destroy(session)
        finally:
            core.mpn_lexicon_destroy(lexicon)
    # Synthetic vocabulary proves the rule is about decoding alternatives at
    # a boundary, not any production phrase or historical candidate cache.
    fixture = ('MEOWPUUT-ZHUYIN-1\n'
               '18\t183\t乙\t乙\t10000\n'
               'ji\tji3\t甲\t甲\t10000\n'
               'ulg\tul4g7\t丙丁\t丙丁\t10000\n'
               'ulg\tul4g7\t戊己\t戊己\t9900\n'
               'ulg\tul4g7\t庚辛\t庚辛\t9800\n'
               'z;\tz;4\t放\t放\t9000\n').encode()
    lexicon = core.mpn_lexicon_create(fixture, len(fixture))
    assert lexicon
    try:
        for ending in ('丙丁', '戊己', '庚辛'):
            session = core.mpn_session_create(lexicon)
            try:
                prefix = 'ji3183ul4g7'
                target = ('甲乙' + ending, len(prefix))
                enter(session, prefix)
                assert target in candidates(session), ('synthetic fixture', target)
                enter(session, 'z')
                current = candidates(session)
                if target not in current:
                    failures.append(('synthetic alternative lost', target, current))
                else:
                    assert core.mpn_choose(session, current.index(target))
                    assert core.mpn_committed(session).decode() == target[0] and core.mpn_input(session) == b'z'
                core.mpn_clear(session)
                enter(session, prefix)
                assert core.mpn_key(session, 0xff08)
                enter(session, '4z')
                assert all(text != target[0] for text, _ in candidates(session)), 'changed tone must not retain a stale homophone'
            finally:
                core.mpn_session_destroy(session)
    finally:
        core.mpn_lexicon_destroy(lexicon)
    # Additional homophones must not take the old leading split slots when
    # only one split exists, shifting the fourth/fifth complete sentence down.
    rows = [f'nihao\tni3hao3\t同{char}\t同{char}\t{10000 - i}\n'
            for i, char in enumerate('甲乙丙丁戊')]
    fixture = ('MEOWPUUT-PINYIN-1\n' + ''.join(rows) + 'wo\two3\t我\t我\t10000\n').encode()
    lexicon = core.mpn_lexicon_create(fixture, len(fixture))
    assert lexicon
    session = core.mpn_session_create(lexicon)
    assert session
    try:
        enter(session, 'nihaowo')
        current = candidates(session)
        whole_positions = [i for i, (_, used) in enumerate(current) if used == 7]
        assert whole_positions == [0, 1, 2, 4, 5], ('prefix alternatives displaced complete sentences', current)
        assert ('同乙', 5) in current and ('同丙', 5) in current
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)
    for failure in failures:
        print('FAIL:', failure)
    assert not failures, f'{len(failures)} prefix alternatives disappeared during input'
    first_page = sum(index < 7 for _, _, index, _ in availability)
    print(f'PASS: 12 pinyin/zhuyin script traces retain selectable homophones and exact raw suffixes; {first_page}/12 final targets in first 7, all 12 selectable')
