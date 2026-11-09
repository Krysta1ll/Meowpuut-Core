#!/usr/bin/env python3
"""Exercise long Zhuyin composition, prefix selection, and raw-key retention."""
import ctypes
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'

with tempfile.TemporaryDirectory(prefix='meowpuut-zhuyin-long-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    shared = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *shared,
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    pointer, string, integer, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    for name, args, result in [
        ('mpn_lexicon_create', [string, size], pointer),
        ('mpn_lexicon_destroy', [pointer], None),
        ('mpn_session_create', [pointer], pointer),
        ('mpn_session_destroy', [pointer], None),
        ('mpn_set_simplified', [pointer, integer], None),
        ('mpn_clear', [pointer], None),
        ('mpn_key', [pointer, ctypes.c_int32], integer),
        ('mpn_candidate_count', [pointer], size),
        ('mpn_candidate', [pointer, size], string),
        ('mpn_candidate_consumed', [pointer, size], size),
        ('mpn_choose', [pointer, size], integer),
        ('mpn_input', [pointer], string),
        ('mpn_committed', [pointer], string),
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result

    data = (resources / 'native-zhuyin.tsv').read_bytes()
    lexicon = core.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = core.mpn_session_create(lexicon)
    assert session
    # 「我們今天一起去看電影」, repeated to cross the 64-key boundary.
    one = 'ji3ap7rup wu0 u fu3fm4d042u04u/3'
    written = ('我們今天一起去看電影', '我们今天一起去看电影')
    elapsed = []
    try:
        for simplified in (0, 1):
            core.mpn_set_simplified(session, simplified)
            for with_tones in (True, False):
                reading = one if with_tones else ''.join(c for c in one if c not in ' 6347')
                full = reading * 12
                # Verify every key across the old 64-key cutoff, not merely
                # completed syllables. The composition must remain selectable.
                core.mpn_clear(session)
                for offset, char in enumerate(full[:120], 1):
                    assert core.mpn_key(session, ord(char)), (with_tones, offset, char)
                    assert core.mpn_input(session).decode() == full[:offset]
                    if offset <= 64:
                        continue
                    visible = [core.mpn_candidate(session, i).decode()
                               for i in range(min(7, core.mpn_candidate_count(session)))]
                    assert any(len(text) >= 2 and (written[simplified] * 12).startswith(text)
                               for text in visible), (simplified, with_tones, offset, visible)
                # Include prefixes just over the original limit and beyond 100 keys.
                for length in (65, 68, 71, 80, 100, min(len(full) - 1, 120)):
                    raw = full[:length]
                    core.mpn_clear(session)
                    for char in raw:
                        start = time.perf_counter_ns()
                        assert core.mpn_key(session, ord(char)), (with_tones, raw, char)
                        elapsed.append((time.perf_counter_ns() - start) / 1_000_000)
                    visible = [core.mpn_candidate(session, i).decode() for i in range(min(7, core.mpn_candidate_count(session)))]
                    coherent = [(i, text) for i, text in enumerate(visible)
                                if len(text) >= 2 and (written[simplified] * 6).startswith(text)]
                    assert coherent, (simplified, with_tones, length, visible)
                    index, selected = coherent[0]
                    consumed = core.mpn_candidate_consumed(session, index)
                    assert 0 < consumed <= len(raw), (raw, selected, consumed)
                    assert core.mpn_choose(session, index)
                    assert core.mpn_committed(session).decode() == selected
                    assert core.mpn_input(session).decode() == raw[consumed:], (raw, consumed)
                # The current engine accepts at most 256 raw keys. Keep all
                # of them when a segment is selected, including the boundary.
                core.mpn_clear(session)
                raw = full[:256]
                for char in raw:
                    assert core.mpn_key(session, ord(char))
                assert core.mpn_input(session).decode() == raw
                visible = [core.mpn_candidate(session, i).decode()
                           for i in range(min(7, core.mpn_candidate_count(session)))]
                coherent = [(i, text) for i, text in enumerate(visible)
                            if len(text) >= 2 and (written[simplified] * 12).startswith(text)
                            and 0 < core.mpn_candidate_consumed(session, i) <= 64]
                assert coherent, (simplified, with_tones, 256, visible)
                index, selected = coherent[0]
                consumed = core.mpn_candidate_consumed(session, index)
                assert 0 < consumed <= 64
                assert core.mpn_choose(session, index)
                assert core.mpn_committed(session).decode() == selected
                assert core.mpn_input(session).decode() == raw[consumed:]
        print('PASS: long Zhuyin prefixes preserve selectable phrases and exact suffixes in both scripts')
        print(f'BENCH: {len(elapsed)} keystrokes, median {statistics.median(elapsed):.3f} ms, P95 {statistics.quantiles(elapsed, n=20)[18]:.3f} ms (local)')
    finally:
        core.mpn_session_destroy(session)
        core.mpn_lexicon_destroy(lexicon)
