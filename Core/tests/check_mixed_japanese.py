#!/usr/bin/env python3
"""Portable mixed-input boundary, evidence and bounded-work regression."""
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
with tempfile.TemporaryDirectory() as directory:
    library = Path(directory) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC', str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    def bind(name, result, *arguments):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
        return function
    pointer, text, number = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int
    create_lexicon = bind('mpn_lexicon_create', pointer, text, ctypes.c_size_t)
    destroy_lexicon = bind('mpn_lexicon_destroy', None, pointer)
    create = bind('mpn_session_create', pointer, pointer)
    destroy = bind('mpn_session_destroy', None, pointer)
    add = bind('mpn_add_japanese_word', number, pointer, text, text, number)
    mixed = bind('mpn_mixed_pinyin', text, pointer, text, number)
    prefix = bind('mpn_mixed_pinyin_prefix', text, pointer, text, number, ctypes.POINTER(ctypes.c_size_t))
    key = bind('mpn_key', number, pointer, ctypes.c_int32)
    get_input = bind('mpn_input', text, pointer)
    set_context = bind('mpn_set_segment_context', None, pointer, text)
    get_context = bind('mpn_segment_context', text, pointer)
    clear = bind('mpn_clear', None, pointer)
    get_candidate_count = bind('mpn_candidate_count', ctypes.c_size_t, pointer)
    get_candidate = bind('mpn_candidate', text, pointer, ctypes.c_size_t)
    get_consumed = bind('mpn_candidate_consumed', ctypes.c_size_t, pointer, ctypes.c_size_t)
    choose = bind('mpn_choose', number, pointer, ctypes.c_size_t)
    get_commit = bind('mpn_committed', text, pointer)
    set_simplified = bind('mpn_set_simplified', None, pointer, number)
    # The public lexicon and registration APIs accept positive int weights.
    # Several valid fragments must not wrap the accumulated ranking score.
    for chinese_weight, japanese_weight in [(4000, 4000), (2147483647, 4000),
                                            (4000, 2147483647), (2147483647, 2147483647)]:
        fixture = (f'MEOWPUUT-PINYIN-2\nwo\two3\t我\t我\t{chinese_weight}\t{chinese_weight}\n').encode()
        owner = create_lexicon(fixture, len(fixture))
        assert owner
        bounded = create(owner)
        assert bounded
        try:
            assert add(bounded, b'suki', '好き'.encode(), japanese_weight)
            assert key(bounded, ord('w'))
            set_context(bounded, '你好'.encode())
            for hans in (0, 1):
                for fragments in (1, 2, 3):
                    raw = b'wo' + b'sukiwo' * fragments
                    expected = '我' + '好き我' * fragments
                    assert mixed(bounded, raw, hans).decode() == expected, (chinese_weight, japanese_weight, raw)
                    used = ctypes.c_size_t()
                    assert prefix(bounded, raw + b'x', hans, ctypes.byref(used)).decode() == expected
                    assert used.value == len(raw)
                    assert get_input(bounded) == b'w' and get_context(bounded).decode() == '你好'
        finally:
            destroy(bounded)
            destroy_lexicon(owner)
    print('PASS: mixed score bounds, one to three fragments, both scripts, prefix counts and active state')
    resources = Path(os.environ.get('MEOWPUUT_MIXED_RESOURCES', root / '.build/Meowpuut.app/Contents/Resources'))
    data = (resources / 'native-pinyin.tsv').read_bytes()
    lexicon = create_lexicon(data, len(data))
    assert lexicon
    session = create(lexicon)
    assert session
    words = [('suki', '好き', 6000), ('haoki', '好き', 5000), ('shiberu', '食べる', 5200),
             ('sushi', '寿司', 5700), ('ramen', 'ラーメン', 5700), ('ongaku', '音楽', 5200),
             ('yinyue', '音楽', 3500), ('au', '会う', 5500), ('kirei', '綺麗', 8000)]
    for reading, word, weight in words:
        assert add(session, reading.encode(), word.encode(), weight), reading
    assert not add(session, b'ai', '愛'.encode(), 7000)
    assert not add(session, b'au', '会う'.encode(), 3500)
    for raw, expected in [
        ('wohaokini', '我好き你'), ('wohaokiyinyue', '我好き音楽'),
        ('woxiangshiberusushiheramen', '我想食べる寿司和ラーメン'),
        ('woyaoauni', '我要会う你'),
    ]:
        actual = mixed(session, raw.encode(), 0).decode()
        assert actual == expected, (raw, actual, expected)
        print('PASS:', raw, '→', actual)
    assert mixed(session, b'wohaokiyinyue', 1).decode() == '我好き音楽'
    for reading in [b'woxihuanyinyue', b'woxihuanni', b'womenjintianqukandianying', b'programming', b'audio', b'auto', b'autumn', b'author', b'authentication', b'automobile']:
        assert mixed(session, reading, 0) == b'', ('must not invent Japanese', reading)
    consumed = ctypes.c_size_t(999)
    assert prefix(session, b'aunihao', 0, ctypes.byref(consumed)).decode() == '会う'
    assert consumed.value == 2 and b'aunihao'[consumed.value:] == b'nihao'
    assert prefix(session, b'yinyuenihao', 0, ctypes.byref(consumed)) == b'' and consumed.value == 0
    assert prefix(session, b'yinyuesukinihao', 0, ctypes.byref(consumed)).decode() == '音楽'
    assert consumed.value == len(b'yinyue') and b'yinyuesukinihao'[consumed.value:] == b'sukinihao'
    assert prefix(session, b'wosukinihao', 0, ctypes.byref(consumed)).decode() == '我好き'
    assert consumed.value == len(b'wosuki') and b'wosukinihao'[consumed.value:] == b'nihao'
    assert get_consumed(None, 0) == 0 and get_consumed(session, 999999) == 0
    for hans in (0, 1):
        set_simplified(session, hans)
        for chinese, phrase, endings in [
            ('jinwandeyueliang', '今晚的月亮', ('ki', 'kir', 'kire')),
            ('woxihuan', '我喜欢' if hans else '我喜歡', ('sus', 'sush')),
        ]:
            for ending in endings:
                clear(session)
                raw = (chinese + ending).encode()
                for byte in raw:
                    assert key(session, byte), raw
                candidates = [get_candidate(session, i).decode() for i in range(get_candidate_count(session))]
                assert phrase in candidates[:7], (raw, candidates[:7])
                index = candidates.index(phrase)
                assert get_consumed(session, index) == len(chinese), (raw, index, get_consumed(session, index))
                assert choose(session, index)
                assert get_commit(session).decode() == phrase and get_input(session).decode() == ending
        clear(session)
    set_simplified(session, 0)
    for byte in b'woxihuanni': assert key(session, byte)
    candidates = [get_candidate(session, i).decode() for i in range(get_candidate_count(session))]
    assert candidates[0] == '我喜歡你', candidates[:5]
    set_context(session, '我喜'.encode())
    prior = get_context(session)
    prior_input = get_input(session)
    assert mixed(session, b'wohaokini', 0).decode() == '我好き你'
    assert get_input(session) == prior_input and get_context(session) == prior
    clear(session)
    assert not get_context(session) and not get_input(session)
    with open(resources / 'native-japanese.json', encoding='utf-8') as source:
        dictionary = json.load(source)
    # Mirrors the platform adapters: register each reading's highest-ranked
    # word. The optional cases become active when the curated data is present.
    real = create(lexicon)
    for reading, entries in sorted(dictionary.items()):
        if entries:
            entry = entries[0]
            add(real, reading.encode(), entry['text'].encode(), entry['weight'])
    if all(reading in dictionary for reading in ['haoki', 'yinyue', 'shiberu', 'au']):
        for raw, expected in [('wohaokini', '我好き你'), ('wohaokiyinyue', '我好き音楽'),
                              ('woxiangshiberusushiheramen', '我想食べる寿司和ラーメン'),
                              ('woyaoauni', '我要会う你')]:
            actual = mixed(real, raw.encode(), 0).decode()
            assert actual == expected, ('generated Japanese lexicon', raw, actual, expected)
        for raw in [b'woxihuanyinyue', b'woxihuanni', b'zhongguoren', b'audio', b'auto', b'autumn', b'author']:
            assert not mixed(real, raw, 0), ('generated lexicon produced spurious Japanese', raw, mixed(real, raw, 0))
        print('PASS: mixed-input checks with the generated Japanese lexicon')
    destroy(real)
    # Stress repeated occurrences and a full-sized dictionary; report timing,
    # without asserting a hardware-dependent limit.
    stress = create(lexicon)
    for i in range(512):
        assert add(stress, b'suki', ('好き' + str(i)).encode(), 6000)
    for label, func, reading in [('whole', lambda r: mixed(stress, r, 0), b'wo' + b'suki' * 19 + b'ni'),
                                 ('prefix', lambda r: prefix(stress, r, 0, ctypes.byref(consumed)), b'qq' + b'suki' * 19 + b'ni')]:
        timings = []
        for _ in range(40):
            start = time.perf_counter()
            func(reading)
            timings.append((time.perf_counter() - start) * 1000)
        timings.sort()
        print(f'BENCH {label} 512 entries, 80 bytes: median {statistics.median(timings):.3f} ms; P95 {timings[int(len(timings)*.95)-1]:.3f} ms')
    sequence = b'wo' + b'suki' * 19 + b'ni'
    steps = []
    for end in range(1, len(sequence) + 1):
        start = time.perf_counter()
        mixed(stress, sequence[:end], 0)
        prefix(stress, sequence[:end], 0, ctypes.byref(consumed))
        steps.append((time.perf_counter() - start) * 1000)
    steps.sort()
    print(f'BENCH mixed+prefix per key, 512 entries: median {statistics.median(steps):.3f} ms; P95 {steps[int(len(steps)*.95)-1]:.3f} ms')
    destroy(stress)
    destroy(session)
    destroy_lexicon(lexicon)
    print('PASS: evidence, three fragments, consumed suffix, context isolation and short reading')
