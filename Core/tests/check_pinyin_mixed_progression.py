#!/usr/bin/env python3
"""Exercise long Pinyin/mixed prefixes with the production lexicons."""
import ctypes
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

root = Path(__file__).resolve().parents[2]
resources = Path(os.environ.get('MEOWPUUT_MIXED_RESOURCES', root / '.build/Meowpuut.app/Contents/Resources'))
with tempfile.TemporaryDirectory() as directory:
    library = Path(directory) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC', str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    def bind(name, result, *args):
        func = getattr(core, name)
        func.restype, func.argtypes = result, args
        return func
    P, S, N, Z = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    make_lexicon = bind('mpn_lexicon_create', P, S, Z)
    destroy_lexicon = bind('mpn_lexicon_destroy', None, P)
    create = bind('mpn_session_create', P, P)
    destroy = bind('mpn_session_destroy', None, P)
    add = bind('mpn_add_japanese_word', N, P, S, S, N)
    key = bind('mpn_key', N, P, ctypes.c_int32)
    clear = bind('mpn_clear', None, P)
    set_hans = bind('mpn_set_simplified', None, P, N)
    count = bind('mpn_candidate_count', Z, P)
    candidate = bind('mpn_candidate', S, P, Z)
    candidate_length = bind('mpn_candidate_consumed', Z, P, Z)
    choose = bind('mpn_choose', N, P, Z)
    get_input = bind('mpn_input', S, P)
    get_commit = bind('mpn_committed', S, P)
    clear_commit = bind('mpn_clear_commit', None, P)
    mixed = bind('mpn_mixed_pinyin', S, P, S, N)
    mixed_prefix = bind('mpn_mixed_pinyin_prefix', S, P, S, N, ctypes.POINTER(Z))
    get_context = bind('mpn_segment_context', S, P)
    set_context = bind('mpn_set_segment_context', None, P, S)
    data = (resources / 'native-pinyin.tsv').read_bytes()
    lexicon = make_lexicon(data, len(data))
    assert lexicon
    session = create(lexicon)
    assert session
    dictionary = json.loads((resources / 'native-japanese.json').read_text(encoding='utf-8'))
    for reading, entries in sorted(dictionary.items()):
        if entries:
            entry = entries[0]
            add(session, reading.encode(), entry['text'].encode(), entry['weight'])

    def enter(raw, hans=0):
        clear(session)
        clear_commit(session)
        set_hans(session, hans)
        for byte in raw.encode():
            assert key(session, byte), raw
        return [candidate(session, i).decode() for i in range(count(session))]

    for hans in (0, 1):
        for raw, phrase, remaining in [
            ('jinwanwoyaochiu', '今晚我要吃', 'u'),
            ('jinwanwoyaochiud', '今晚我要吃', 'ud'),
            ('jinwanwoyaochiudo', '今晚我要吃', 'udo'),
            ('womenjintianqukandianying' * 3 + 'kir',
             ('我们今天去看电影' if hans else '我們今天去看電影') * 3, 'kir'),
            ('womenjintianqukandianying' * 3 + 'k',
             ('我们今天去看电影' if hans else '我們今天去看電影') * 3, 'k'),
        ]:
            choices = enter(raw, hans)
            assert phrase in choices[:7], (raw, choices[:7])
            index = choices.index(phrase)
            assert candidate_length(session, index) == len(raw) - len(remaining), raw
            assert choose(session, index)
            assert get_commit(session).decode() == phrase
            assert get_input(session).decode() == remaining
        set_hans(session, hans)
        for raw, expected in [
            ('jinwanwoyaochiudon', '今晚我要吃うどん'),
            ('woyaochisushi', '我要吃寿司'),
            ('woxihuanchiheudon', ('我喜欢' if hans else '我喜歡') + '血和うどん'),
            ('woxihuansushihenhaochi', ('我喜欢' if hans else '我喜歡') + '寿司很好吃'),
            ('woxihuansushidechi', ('我喜欢' if hans else '我喜歡') + '寿司的血'),
            ('woxihuansushichiheudon', ('我喜欢' if hans else '我喜歡') + '寿司血和うどん'),
            ('woxihuankirei', ('我喜欢' if hans else '我喜歡') + '綺麗'),
        ]:
            assert mixed(session, raw.encode(), hans).decode() == expected, raw
        for raw, expected, consumed in [
            ('wosukinih', '我好き你', len('wosukini')),
            ('wosukinihao', '我好き', len('wosuki')),
            ('sukinihao', '好き', len('suki')),
        ]:
            length = Z()
            before_input, before_context = get_input(session), get_context(session)
            assert mixed_prefix(session, raw.encode(), hans, ctypes.byref(length)).decode() == expected, raw
            assert length.value == consumed and raw[length.value:], (raw, length.value)
            assert get_input(session) == before_input and get_context(session) == before_context
        # The mixed-prefix lookup must not disappear when the entire reading
        # crosses the bounded decoder window; the unselected suffix is intact.
        for length in (81, 128, 255):
            raw = ('wosukinihao' + 'nihao' * 50)[:length]
            length_out = Z()
            result = mixed_prefix(session, raw.encode(), hans, ctypes.byref(length_out)).decode()
            assert result == '我好き' and length_out.value == len('wosuki'), (length, result, length_out.value)
            assert raw[length_out.value:] == raw[len('wosuki'):]
        # A valid full syllable ending in ng stays fully consumed; it must not
        # be mistaken for an incomplete g merely because the input is long.
        raw = 'womenjintianqukandianying' * 3 + 'peng'
        choices = enter(raw, hans)
        assert choices and candidate_length(session, 0) == len(raw), (raw, choices[:3])
    for raw in ('woxihuanni', 'zhongguoren', 'audio', 'auto', 'author'):
        assert not mixed(session, raw.encode(), 0), raw
    print('PASS: Pinyin/Mixed progression, preserved suffixes, long readings and Chinese/Japanese ambiguity')

    # Report cost rather than using a machine-specific pass threshold.
    for label, raw in [
        ('natural-long', ('womenjintianqukandianying' * 10 + 'kir')[:255]),
        ('repeated-initials', ('zhuang' * 42 + 'kir')[:255]),
    ]:
        clear(session)
        durations = []
        for byte in raw.encode():
            start = time.perf_counter()
            assert key(session, byte)
            durations.append((time.perf_counter() - start) * 1000)
        durations.sort()
        print(f'BENCH {label}: median {statistics.median(durations):.3f} ms; P95 {durations[int(len(durations)*.95)-1]:.3f} ms')
    reading = ('wosukinihao' + 'nihao' * 50)[:255]
    durations = []
    for length in range(81, len(reading) + 1):
        actual_consumed = Z()
        start = time.perf_counter()
        result = mixed_prefix(session, reading[:length].encode(), 0, ctypes.byref(actual_consumed)).decode()
        durations.append((time.perf_counter() - start) * 1000)
        assert result == '我好き' and actual_consumed.value == len('wosuki')
    durations.sort()
    print(f'BENCH mixed prefix beyond 80: median {statistics.median(durations):.3f} ms; P95 {durations[int(len(durations)*.95)-1]:.3f} ms')
    destroy(session)
    destroy_lexicon(lexicon)
