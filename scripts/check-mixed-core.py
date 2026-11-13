#!/usr/bin/env python3
"""Exercise the portable core without the Swift or platform adapters."""
import ctypes
import json
import os
import subprocess
import sys
import time
import statistics
from pathlib import Path

root = Path(__file__).resolve().parent.parent
resources = root / '.build/lexicon'
if True:
    env = {**os.environ, 'PATH': f"{Path.home() / '.cargo/bin'}:{os.environ.get('PATH', '')}"}
    subprocess.run(['cargo', 'build', '--release', '-p', 'meowpuut-ffi'], cwd=root, env=env, check=True)
    ext = 'dylib' if sys.platform == 'darwin' else ('dll' if sys.platform == 'win32' else 'so')
    prefix_name = '' if sys.platform == 'win32' else 'lib'
    lib = root / 'target/release' / f'{prefix_name}meowpuut_ffi.{ext}'
    core = ctypes.CDLL(str(lib))
    def signature(name, result, *arguments):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
        return function
    ptr, text, number = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int
    create_lexicon = signature('mpn_lexicon_create', ptr, text, ctypes.c_size_t)
    destroy_lexicon = signature('mpn_lexicon_destroy', None, ptr)
    create_session = signature('mpn_session_create', ptr, ptr)
    destroy_session = signature('mpn_session_destroy', None, ptr)
    add_word = signature('mpn_add_japanese_word', number, ptr, text, text, number)
    mixed = signature('mpn_mixed_pinyin', text, ptr, text, number)
    prefix = signature('mpn_mixed_pinyin_prefix', text, ptr, text, number, ctypes.POINTER(ctypes.c_size_t))
    key = signature('mpn_key', number, ptr, ctypes.c_int32)
    input_text = signature('mpn_input', text, ptr)
    clear = signature('mpn_clear', None, ptr)
    count = signature('mpn_candidate_count', ctypes.c_size_t, ptr)
    candidate = signature('mpn_candidate', text, ptr, ctypes.c_size_t)
    data = (resources / 'native-pinyin.tsv').read_bytes()
    lexicon = create_lexicon(data, len(data))
    assert lexicon
    session = create_session(lexicon)
    assert session
    for reading, entries in json.loads((resources / 'native-japanese.json').read_text()).items():
        if entries:
            entry = entries[0]
            add_word(session, reading.encode(), entry['text'].encode(), entry['weight'])
    # Fixture keeps the test independent of installed Japanese lexicon versions.
    assert add_word(session, b'hajimemashite', 'はじめまして'.encode(), 6000)
    for raw, expected in [
        ('woxihuannidenekohenkeai', '我喜歡你的猫很可愛'),
        ('woxihuansushiheramen', '我喜歡寿司和ラーメン'),
        ('woxiangshuohajimemashite', '我想說はじめまして'),
    ]:
        result = mixed(session, raw.encode(), 0).decode()
        assert result == expected, (raw, result, expected)
        print(f'PASS: {raw} → {result}')
    assert mixed(session, b'nihaosuki', 1).decode() == '你好好き'
    assert mixed(session, b'neko', 0) == b''  # no invented Chinese
    for raw in [b'woxihuanni', b'womenjintianqukandianying', b'zhongguoren', b'jintiantianqihenhao',
                b'programming', b'minecraft', b'sushiswap', b'sukiyaki', b'nekowebsite']:
        assert mixed(session, raw, 0) == b'', (raw, 'must not invent a mixed candidate')
    assert add_word(session, b's', 'す'.encode(), 7000) == 0
    assert add_word(session, b'bad!', 'bad'.encode(), 7000) == 0
    assert add_word(session, b'weak', '弱'.encode(), 3999) == 0
    for raw, expected in [('qinchu', '清楚'), ('jingzhang', '緊張'), ('gognzuo', '工作')]:
        clear(session)
        for ch in raw.encode(): assert key(session, ch)
        choices = [candidate(session, i).decode() for i in range(count(session))]
        assert expected in choices, (raw, expected, choices[:10])
        if raw == 'qinchu': assert choices[0] != expected, 'exact reading must remain ahead of correction'
    clear(session)
    consumed = ctypes.c_size_t()
    assert prefix(session, b'wosukinihao', 0, ctypes.byref(consumed)).decode() == '我好き'
    assert consumed.value == len('wosuki')
    assert prefix(session, b'sukinihao', 0, ctypes.byref(consumed)).decode() == '好き'
    assert consumed.value == len('suki')
    assert prefix(session, b'neko', 0, ctypes.byref(consumed)) == b'' and consumed.value == 0
    assert prefix(session, b'wosukinihao', 0, ctypes.byref(consumed)).decode() == '我好き'
    assert b'wosukinihao'[consumed.value:] == b'nihao'
    key(session, ord('n'))
    mixed(session, b'wosukini', 0)
    assert input_text(session) == b'n'  # lookup must not mutate composition
    for reading in [b'woxihuansushiheramen' * 4, b'neko' * 19, b'sukisukisukisukisukisukisuki']:
        timings = []
        for _ in range(30):
            start = time.perf_counter()
            mixed(session, reading, 0)
            timings.append((time.perf_counter() - start) * 1000)
        print(f'BENCH mixed {len(reading)} bytes: median {statistics.median(timings):.3f} ms; max {max(timings):.3f} ms')
    destroy_session(session)
    destroy_lexicon(lexicon)
    print('PASS: simplified, Japanese-only and session isolation')
