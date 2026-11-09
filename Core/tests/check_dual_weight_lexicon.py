#!/usr/bin/env python3
"""Check V1 compatibility and per-script ordering in V2 lexicons."""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
POINTER = ctypes.c_void_p
SIZE = ctypes.c_size_t
TEXT = ctypes.c_char_p


class CandidateInfo(ctypes.Structure):
    _fields_ = [('kind', ctypes.c_uint32), ('score_kind', ctypes.c_uint32),
                ('consumed', ctypes.c_uint64), ('remaining', ctypes.c_uint64),
                ('score', ctypes.c_double)]


def fixture(mode, version, rows):
    rows = sorted(rows, key=lambda row: (row[0], -row[4]))
    lines = [f'MEOWPUUT-{mode.upper()}-{version}']
    for row in rows:
        lines.append('\t'.join(map(str, row[:5 if version == 1 else 6])))
    return ('\n'.join(lines) + '\n').encode()


with tempfile.TemporaryDirectory(prefix='meowpuut-dual-weights-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags,
                    str(ROOT / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ('mpn_lexicon_create', [TEXT, SIZE], POINTER),
        ('mpn_lexicon_destroy', [POINTER], None),
        ('mpn_session_create', [POINTER], POINTER),
        ('mpn_session_destroy', [POINTER], None),
        ('mpn_set_simplified', [POINTER, ctypes.c_int], None),
        ('mpn_set_segment_context', [POINTER, TEXT], None),
        ('mpn_clear', [POINTER], None),
        ('mpn_clear_commit', [POINTER], None),
        ('mpn_key', [POINTER, ctypes.c_int32], ctypes.c_int),
        ('mpn_page', [POINTER], SIZE),
        ('mpn_candidate_count', [POINTER], SIZE),
        ('mpn_candidate', [POINTER, SIZE], TEXT),
        ('mpn_candidate_consumed', [POINTER, SIZE], SIZE),
        ('mpn_candidate_info', [POINTER, SIZE, ctypes.POINTER(CandidateInfo), SIZE], ctypes.c_int),
        ('mpn_choose', [POINTER, SIZE], ctypes.c_int),
        ('mpn_input', [POINTER], TEXT),
        ('mpn_preedit', [POINTER], TEXT),
        ('mpn_committed', [POINTER], TEXT),
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result

    def load(data):
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, data[:100]
        session = core.mpn_session_create(lexicon)
        assert session
        return lexicon, session

    def enter(session, text):
        for char in text:
            assert core.mpn_key(session, ord(char)), (text, char)

    def candidates(session):
        return [core.mpn_candidate(session, i).decode()
                for i in range(core.mpn_candidate_count(session))]

    def info(session, index):
        item = CandidateInfo()
        assert core.mpn_candidate_info(session, index, ctypes.byref(item), ctypes.sizeof(item))
        return item

    # A bucket with eight alternatives forces separate first-page and beam
    # decisions; the two scores deliberately rank in opposite directions.
    characters = '一二三四五六七八'
    for mode, bare, pronounced, second_bare, second_reading in [
        ('pinyin', 'ma', 'ma1', 'wo', 'wo3'),
        ('zhuyin', '12', '12 ', 'q', 'q '),
    ]:
        rows = [(bare, pronounced, char, char, 10000 - i * 700, 1000 + i * 700)
                for i, char in enumerate(characters)]
        rows.append((second_bare, second_reading, '我', '我', 8000, 8000))

        # Old five-field resources keep precisely the same order for both
        # modes, including metadata and multiword decoding.
        lexicon, session = load(fixture(mode, 1, rows))
        try:
            for simplified in (0, 1):
                core.mpn_clear(session)
                core.mpn_set_simplified(session, simplified)
                enter(session, bare)
                assert candidates(session)[:8] == list(characters), (mode, simplified, candidates(session)[:8])
                assert info(session, 0).score == 10000
                core.mpn_clear(session)
                enter(session, bare + second_bare)
                assert candidates(session)[0] == '一我', (mode, simplified, candidates(session)[:7])
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

        lexicon, session = load(fixture(mode, 2, rows))
        try:
            for simplified in (0, 1):
                expected = list(characters[::-1] if simplified else characters)
                core.mpn_clear(session)
                core.mpn_set_simplified(session, simplified)
                enter(session, bare)
                assert candidates(session)[:8] == expected, (mode, simplified, candidates(session)[:8])
                score = 5900 if simplified else 10000
                first = info(session, 0)
                assert first.kind == 1 and first.score_kind == 1 and first.score == score, (mode, simplified, first.score)
                assert first.consumed == first.remaining == len(bare)
                assert core.mpn_key(session, 0xff56) and core.mpn_page(session) == 1
                index = candidates(session).index(expected[7])
                assert index == 7 and core.mpn_choose(session, index)
                assert core.mpn_committed(session).decode() == expected[7]
                assert not core.mpn_input(session)

                # The beam must use the selected script's score before it
                # truncates alternatives, not merely reorder final results.
                core.mpn_clear(session)
                enter(session, bare + second_bare)
                assert candidates(session)[0] == expected[0] + '我', (mode, simplified, candidates(session)[:7])

                # An unfinished single initial uses the same script ranking.
                core.mpn_clear(session)
                enter(session, bare[:1])
                assert candidates(session)[0] == expected[0], (mode, simplified, candidates(session)[:7])
                assert core.mpn_candidate_consumed(session, 0) == 1

            # A script change refreshes candidates without losing the active
            # reading or text the host has not taken from a previous choice.
            core.mpn_clear(session)
            core.mpn_set_simplified(session, 0)
            enter(session, second_bare)
            assert core.mpn_choose(session, candidates(session).index('我'))
            assert core.mpn_committed(session).decode() == '我'
            enter(session, bare)
            before = core.mpn_preedit(session)
            core.mpn_set_simplified(session, 1)
            assert core.mpn_input(session).decode() == bare and core.mpn_preedit(session) == before
            assert core.mpn_committed(session).decode() == '我'
            assert candidates(session)[0] == '八'
            index = candidates(session).index('七')
            assert core.mpn_choose(session, index)
            assert core.mpn_committed(session).decode() == '我七'
            assert not core.mpn_input(session)
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

    # Two traditional variants can map to one simplified output. Deduplication
    # must use the best score of the current script, not whichever V1 row was
    # encountered first in traditional order.
    collision = [
        ('ji', 'ji1', '甲', '甲', 9000, 1000),
        ('ji', 'ji1', '丙', '丙', 5000, 5000),
        ('ji', 'ji1', '乙', '甲', 1000, 8000),
    ]
    lexicon, session = load(fixture('pinyin', 2, collision))
    try:
        enter(session, 'ji')
        assert candidates(session)[:3] == ['甲', '丙', '乙']
        core.mpn_set_simplified(session, 1)
        assert candidates(session)[:2] == ['甲', '丙']
        assert candidates(session).count('甲') == 1
        assert info(session, 0).score == 8000
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)

    # Prefix completion scans a bounded set of dictionary entries per bucket.
    # The best simplified alternative must be considered before that limit.
    completions = [('mei', 'mei1', char, char, 10000 - i * 700, 1000 + i * 700)
                   for i, char in enumerate(characters)]
    lexicon, session = load(fixture('pinyin', 2, completions))
    try:
        for simplified, expected in [(0, '一'), (1, '八')]:
            core.mpn_clear(session)
            core.mpn_set_simplified(session, simplified)
            enter(session, 'me')
            assert candidates(session)[0] == expected, (simplified, candidates(session)[:7])
            assert core.mpn_candidate_consumed(session, 0) == 2
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)

    # Several traditional entries can collapse to one simplified candidate.
    # They must not exhaust the bounded prefix-completion scan before another
    # distinct simplified word is considered.
    duplicates = [('mei', 'mei1', char, '猫', 9000 - i, 9000)
                  for i, char in enumerate('甲乙丙丁戊己庚辛')]
    duplicates.append(('mei', 'mei1', '壬', '们', 8000, 8000))
    lexicon, session = load(fixture('pinyin', 2, duplicates))
    try:
        core.mpn_set_simplified(session, 1)
        enter(session, 'me')
        assert candidates(session)[:2] == ['猫', '们'], candidates(session)[:7]
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)

    # Initial-only predictions also have a bounded index; repeated display
    # forms must not crowd out the next distinct prediction.
    initials = [('mei', 'mei1', chr(0x4e00 + i), '猫', 10000 - i, 9000 - i)
                for i in range(64)]
    initials.append(('mei', 'mei1', '龥', '们', 8000, 5000))
    for version in (1, 2):
        lexicon, session = load(fixture('pinyin', version, initials))
        try:
            core.mpn_set_simplified(session, 1)
            enter(session, 'm')
            assert candidates(session)[:2] == ['猫', '们'], (version, candidates(session)[:7])
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

    # When the suffix is not yet usable, the single-character fallback must
    # also count distinct displayed characters and preserve the raw suffix.
    partials = [('ma', 'ma1', chr(0x4e00 + i), '猫', 10000 - i, 9000 - i)
                for i in range(32)]
    partials.append(('ma', 'ma1', '龥', '们', 8000, 5000))
    for version in (1, 2):
        lexicon, session = load(fixture('pinyin', version, partials))
        try:
            core.mpn_set_simplified(session, 1)
            enter(session, 'maqq')
            assert candidates(session)[:2] == ['猫', '们'], (version, candidates(session)[:7])
            assert core.mpn_candidate_consumed(session, 1) == 2
            assert core.mpn_choose(session, 1)
            assert core.mpn_committed(session).decode() == '们'
            assert core.mpn_input(session) == b'qq'
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

    # Context bonuses must use the current script's evidence, too. Older
    # two-field context tables use their shared score for either script.
    context_rows = [('ma', 'ma1', '一', '一', 8000, 8000),
                    ('ma', 'ma1', '二', '二', 8000, 8000),
                    ('wo', 'wo3', '我', '我', 8000, 8000)]
    for version, context, expected in [
        (1, '一我\t10000\n二我\t1\n', ('一我', '一我')),
        (2, '一我\t10000\n二我\t1\n', ('一我', '一我')),
        (2, '一我\t10000\t1\n二我\t1\t10000\n', ('一我', '二我')),
    ]:
        data = fixture('pinyin', version, context_rows) + ('# context\n' + context).encode()
        lexicon, session = load(data)
        try:
            for script in (0, 1):
                core.mpn_clear(session)
                core.mpn_set_simplified(session, script)
                enter(session, 'mawo')
                assert candidates(session)[0] == expected[script], (version, script, candidates(session)[:7])
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

    for mode, bare, pronounced in [('pinyin', 'ma', 'ma1'), ('zhuyin', '12', '12 ')]:
        rows = [(bare, pronounced, '一', '一', 8000, 8000),
                (bare, pronounced, '二', '二', 8000, 8000)]
        data = fixture(mode, 2, rows) + '# context\n我一\t10000\t1\n我二\t1\t10000\n'.encode()
        lexicon, session = load(data)
        try:
            for simplified, expected in [(0, '一'), (1, '二')]:
                core.mpn_clear(session)
                core.mpn_set_simplified(session, simplified)
                core.mpn_set_segment_context(session, '我'.encode())
                enter(session, bare)
                assert candidates(session)[0] == expected, (mode, simplified, candidates(session)[:7])
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)

    for broken in [
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\t-1\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\tbad\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\textra\t9000\t8000\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\t8000\textra\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\t8000\n# context\n一我\t1\t2\t3\n',
        'MEOWPUUT-PINYIN-2\nma\tma1\t一\t一\t9000\t8000\n# context\n一我\t1\t-1\n',
    ]:
        data = broken.encode()
        lexicon = core.mpn_lexicon_create(data, len(data))
        if lexicon:
            core.mpn_lexicon_destroy(lexicon)
        assert not lexicon, broken

print('PASS: V1/V2 script-specific ordering, metadata, beam, prefixes, selection, and invalid input')
