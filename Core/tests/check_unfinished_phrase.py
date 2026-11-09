#!/usr/bin/env python3
"""Check partial phrase selection with actual compiled Chinese dictionaries.

python3 Core/tests/check_unfinished_phrase.py /path/to/compiled/resources
"""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
with tempfile.TemporaryDirectory(prefix='meowpuut-prefix-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    shared = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *shared, str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_void_p),
        ('mpn_lexicon_destroy', [ctypes.c_void_p], None),
        ('mpn_session_create', [ctypes.c_void_p], ctypes.c_void_p),
        ('mpn_session_destroy', [ctypes.c_void_p], None),
        ('mpn_clear', [ctypes.c_void_p], None),
        ('mpn_set_segment_context', [ctypes.c_void_p, ctypes.c_char_p], None),
        ('mpn_segment_context', [ctypes.c_void_p], ctypes.c_char_p),
        ('mpn_key', [ctypes.c_void_p, ctypes.c_int32], ctypes.c_int),
        ('mpn_set_simplified', [ctypes.c_void_p, ctypes.c_int], None),
        ('mpn_choose', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_int),
        ('mpn_input', [ctypes.c_void_p], ctypes.c_char_p),
        ('mpn_committed', [ctypes.c_void_p], ctypes.c_char_p),
        ('mpn_candidate_count', [ctypes.c_void_p], ctypes.c_size_t),
        ('mpn_candidate', [ctypes.c_void_p, ctypes.c_size_t], ctypes.c_char_p),
    ]:
        function = getattr(core, name)
        function.argtypes, function.restype = args, result

    def candidates(session):
        return [core.mpn_candidate(session, i).decode() for i in range(core.mpn_candidate_count(session))]

    def type_keys(session, keys):
        for key in keys:
            assert core.mpn_key(session, ord(key)), (keys, key)

    def load(mode):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, mode
        session = core.mpn_session_create(lexicon)
        assert session, mode
        return lexicon, session

    lexicon, session = load('pinyin')
    try:
        for simplified in (0, 1):
            core.mpn_set_simplified(session, simplified)
            # Keep multiple meaningful homophones while the next syllable is
            # being typed, and consume only the selected reading prefix.
            for raw, remainder, completion in [('woxih', 'h', 'uan'), ('woxihu', 'hu', 'an'), ('woxihua', 'hua', 'n')]:
                core.mpn_clear(session); type_keys(session, raw)
                visible = candidates(session)
                assert '我喜' in visible[:7] and len(visible) >= 3, (raw, visible[:7])
                assert core.mpn_choose(session, visible.index('我喜'))
                assert core.mpn_committed(session).decode() == '我喜'
                assert core.mpn_input(session).decode() == remainder
                type_keys(session, completion)
                assert ('歡', '欢')[simplified] in candidates(session)[:7], (raw, candidates(session)[:14])
            core.mpn_clear(session); type_keys(session, 'woxihuanni')
            complete = ('我喜歡你', '我喜欢你')[simplified]
            prefix = ('我喜歡', '我喜欢')[simplified]
            visible = candidates(session)
            assert visible[0] == complete and prefix in visible[:7], visible[:7]
            assert core.mpn_choose(session, visible.index(prefix))
            assert core.mpn_input(session) == b'ni'
            assert '你' in candidates(session)[:7]
            for raw, expected, suffix in [('nihaox', '你好', 'x'),
                                          ('woainix', ('我愛你', '我爱你')[simplified], 'x'),
                                          ('jixuz', ('繼續', '继续')[simplified], 'z'),
                                          ('womenjintianquk', ('我們今天去', '我们今天去')[simplified], 'k'),
                                          ('womenjintianq', ('我們今天', '我们今天')[simplified], 'q'),
                                          ('woxihuank', ('我喜歡', '我喜欢')[simplified], 'k'),
                                          ('woxihuannid', ('我喜歡你', '我喜欢你')[simplified], 'd'),
                                          ('nijintianyoukon', '你今天有', 'kon'),
                                          ('jintiantianqih', ('今天天氣', '今天天气')[simplified], 'h')]:
                core.mpn_clear(session); type_keys(session, raw)
                visible = candidates(session)
                assert visible and visible[0] == expected, (raw, visible[:7])
                assert core.mpn_choose(session, 0)
                assert core.mpn_committed(session).decode() == expected
                assert core.mpn_input(session).decode() == suffix, (raw, core.mpn_input(session))
            core.mpn_clear(session); type_keys(session, 'nihaox')
            assert core.mpn_choose(session, 0)
            type_keys(session, 'ihuan')
            assert ('喜歡', '喜欢')[simplified] in candidates(session)[:7]
            core.mpn_clear(session); type_keys(session, 'woxihuannid')
            assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == ('我喜歡你', '我喜欢你')[simplified]
            type_keys(session, 'e')
            assert core.mpn_input(session) == b'de'
            assert '的' in candidates(session)[:7]
            core.mpn_clear(session); type_keys(session, 'womenjintianquk')
            assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == ('我們今天去', '我们今天去')[simplified]
            assert core.mpn_input(session) == b'k'
            type_keys(session, 'andianyingz')
            assert candidates(session)[0] == ('看電影', '看电影')[simplified]
            assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == ('我們今天去看電影', '我们今天去看电影')[simplified]
            assert core.mpn_input(session) == b'z'
            type_keys(session, 'aijian')
            assert ('再見', '再见')[simplified] in candidates(session)[:7]
            assert core.mpn_choose(session, candidates(session).index(('再見', '再见')[simplified]))
            assert core.mpn_committed(session).decode() == ('我們今天去看電影再見', '我们今天去看电影再见')[simplified]
            assert core.mpn_input(session) == b''
            for raw, expected in [('nihao', '你好'), ('nihaoxh', ('你好喜歡', '你好喜欢')[simplified]),
                                  ('nihaoxihuan', ('你好喜歡', '你好喜欢')[simplified]),
                                  ('womenjintianbang', ('我們今天幫', '我们今天帮')[simplified]),
                                  ('zhongwen', '中文'), ('zhongguo', ('中國', '中国')[simplified]),
                                  ('sushi', ('蘇軾', '苏轼')[simplified]), ('wox', ('我想', '我想')[simplified])]:
                core.mpn_clear(session); type_keys(session, raw)
                assert candidates(session)[0] == expected, (raw, candidates(session)[:7])
    finally:
        core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)

    # Session context is limited to a single active partial composition. Hosts
    # that rebuild the same suffix can restore it explicitly after clear.
    for mode, reading in [('pinyin', 'huan'), ('zhuyin', 'cj0 ')]:
        lexicon, session = load(mode)
        try:
            for simplified in (0, 1):
                core.mpn_set_simplified(session, simplified)
                core.mpn_clear(session); type_keys(session, reading)
                baseline = candidates(session)
                core.mpn_clear(session)
                core.mpn_set_segment_context(session, '我喜'.encode())
                assert core.mpn_segment_context(session).decode() == '我喜'
                type_keys(session, reading)
                assert ('歡', '欢')[simplified] in candidates(session)[:7], (mode, candidates(session)[:7])
                core.mpn_clear(session)
                assert not core.mpn_segment_context(session)
                type_keys(session, reading)
                assert candidates(session) == baseline, (mode, candidates(session)[:7], baseline[:7])

                core.mpn_clear(session)
                core.mpn_set_segment_context(session, '我喜'.encode())
                type_keys(session, reading)
                for _ in reading: assert core.mpn_key(session, 0xff08)
                assert core.mpn_input(session) == b'' and not core.mpn_segment_context(session)
                type_keys(session, reading)
                assert candidates(session) == baseline

                core.mpn_clear(session)
                core.mpn_set_segment_context(session, '我喜🐈'.encode())
                assert not core.mpn_segment_context(session)
                type_keys(session, reading)
                assert candidates(session) == baseline
                core.mpn_clear(session)
                core.mpn_set_segment_context(session, b'\xf0\x80\x80\x80')
                assert not core.mpn_segment_context(session)
                core.mpn_set_segment_context(session, '我喜欢'.encode())
                assert core.mpn_segment_context(session).decode() == '我喜歡'
                core.mpn_set_segment_context(session, '這是一個很長的前綴我喜'.encode())
                assert len(core.mpn_segment_context(session).decode()) == 4
                assert core.mpn_segment_context(session).decode().endswith('我喜')
                core.mpn_clear(session)
                core.mpn_set_segment_context(session, ('字' * 30).encode())
                assert not core.mpn_segment_context(session)
                core.mpn_clear(session)
                core.mpn_set_segment_context(session, '我喜'.encode())
                type_keys(session, reading)
                chosen = ('歡', '欢')[simplified]
                assert core.mpn_choose(session, candidates(session).index(chosen))
                assert core.mpn_input(session) == b'' and not core.mpn_segment_context(session)
                # A non-Chinese boundary removes, rather than carries, context.
                core.mpn_set_segment_context(session, '我喜，'.encode())
                assert not core.mpn_segment_context(session)
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)

    # A dictionary's exact abbreviation is still an exact whole-input match;
    # it must not be displaced by the completed prefix of the same reading.
    data = ('MEOWPUUT-PINYIN-1\n'
            'kan\tkan4\t看\t看\t15000\n'
            'wo\two3\t我\t我\t15000\n'
            'woxihuank\two3xi3huan1k1\t我喜歡看\t我喜欢看\t17000\n'
            'xihuan\txi3huan1\t喜歡\t喜欢\t16000\n').encode()
    lexicon = core.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = core.mpn_session_create(lexicon)
    try:
        type_keys(session, 'woxihuank')
        assert candidates(session)[0] == '我喜歡看', candidates(session)[:7]
        assert core.mpn_choose(session, 0)
        assert core.mpn_input(session) == b''
    finally:
        core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)

    lexicon, session = load('zhuyin')
    try:
        for simplified in (0, 1):
            core.mpn_set_simplified(session, simplified)
            core.mpn_clear(session); type_keys(session, 'su3cl3')
            assert candidates(session)[0] == '你好'
            assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == '你好'
            assert core.mpn_input(session) == b''

            for raw, remainder, completion in [('jivuc', 'c', 'j0 '), ('jivucj', 'cj', '0 '), ('ji3vu3c', 'c', 'j0 ')]:
                core.mpn_clear(session); type_keys(session, raw)
                visible = candidates(session)
                assert '我喜' in visible[:7] and len(visible) >= 3, (raw, visible[:7])
                assert core.mpn_choose(session, visible.index('我喜'))
                assert core.mpn_committed(session).decode() == '我喜'
                assert core.mpn_input(session).decode() == remainder
                type_keys(session, completion)
                assert ('歡', '欢')[simplified] in candidates(session)[:7], (raw, candidates(session)[:14])

            # An unfinished next syllable must not reduce a completed phrase to
            # its first character. Its keys stay available for continued input.
            core.mpn_clear(session); type_keys(session, 'su3cl3v')
            assert candidates(session)[0] == '你好', candidates(session)[:7]
            assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == '你好'
            assert core.mpn_input(session) == b'v'
            type_keys(session, 'u3cj0 ')
            assert ('喜歡', '喜欢')[simplified] in candidates(session)[:7]

            # Tone-free prefixes still offer a segment at a decodable syllable
            # boundary; selecting it leaves the next initial untouched.
            core.mpn_clear(session); type_keys(session, 'suclv')
            assert candidates(session)[0] == '你好', candidates(session)[:7]
            assert core.mpn_choose(session, 0)
            assert core.mpn_input(session) == b'v'
            type_keys(session, 'u3cj0 ')
            assert ('喜歡', '喜欢')[simplified] in candidates(session)[:7]

            # A tone following a syllable belongs to that syllable and cannot
            # be retained as an orphaned suffix after selecting a prefix.
            core.mpn_clear(session); type_keys(session, 'su3cl3v')
            assert core.mpn_choose(session, candidates(session).index('你好'))
            assert core.mpn_input(session) == b'v'

            # A fully decoded sentence should still offer completed prefixes on
            # the first page without consuming the unselected suffix.
            core.mpn_clear(session); type_keys(session, 'ji3ap7rup wu0 fm4d042u04u/3')
            full = ('我們今天去看電影', '我们今天去看电影')[simplified]
            prefix = ('我們今天去', '我们今天去')[simplified]
            film = ('看電影', '看电影')[simplified]
            visible = candidates(session)
            assert visible[0] == full, visible[:7]
            assert prefix in visible[:7], visible[:7]
            assert core.mpn_choose(session, visible.index(prefix))
            assert core.mpn_committed(session).decode() == prefix
            assert core.mpn_input(session) == b'd042u04u/3', core.mpn_input(session)
            assert film in candidates(session)[:7], candidates(session)[:7]
            assert core.mpn_choose(session, candidates(session).index(film))
            assert core.mpn_committed(session).decode() == prefix + film
            assert core.mpn_input(session) == b''

            core.mpn_clear(session); type_keys(session, 'ji3ap7rup wu0 fm4d')
            assert candidates(session)[0] == prefix, candidates(session)[:7]
            assert core.mpn_choose(session, 0)
            assert core.mpn_input(session) == b'd'
            type_keys(session, '042u04u/3')
            assert film in candidates(session)[:7], candidates(session)[:7]
    finally:
        core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)

    # Identical displayed text can have distinct readings. A whole-input
    # dictionary match keeps its own consumed-key length when a shorter prefix
    # would display the same text, rather than silently changing selection.
    data = ('MEOWPUUT-ZHUYIN-1\n'
            'sucl\tsu3cl3\t你好\t你好\t15000\n'
            'suclv\tsu3cl3v \t你好\t你好\t16000\n').encode()
    lexicon = core.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = core.mpn_session_create(lexicon)
    try:
        type_keys(session, 'su3cl3v')
        assert candidates(session)[0] == '你好'
        assert core.mpn_choose(session, 0)
        assert core.mpn_input(session) == b''
    finally:
        core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)
print('PASS: unfinished phrase, retained suffix, exact pinyin and zhuyin')
