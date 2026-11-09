#!/usr/bin/env python3
"""Deterministic composition-state invariants, independent of preferred words."""
import ctypes as C
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]


class Info(C.Structure):
    _fields_ = [('kind', C.c_uint32), ('score_kind', C.c_uint32),
                ('consumed', C.c_uint64), ('remaining', C.c_uint64), ('score', C.c_double)]


with tempfile.TemporaryDirectory(prefix='meowpuut-composition-state-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC', '-I' + str(ROOT / 'Core'),
                    os.environ.get('MEOWPUUT_STATE_CORE', str(ROOT / 'Core/MeowpuutCore.cpp')),
                    '-o', str(library)], check=True)
    api = C.CDLL(str(library))
    P, S, N, Z = C.c_void_p, C.c_char_p, C.c_int, C.c_size_t
    for name, result, args in [
        ('lexicon_create', P, [S, Z]), ('lexicon_destroy', None, [P]),
        ('session_create', P, [P]), ('session_destroy', None, [P]),
        ('key', N, [P, N]), ('choose', N, [P, Z]), ('commit', None, [P]),
        ('input', S, [P]), ('preedit', S, [P]), ('committed', S, [P]),
        ('clear', None, [P]), ('clear_commit', None, [P]),
        ('set_simplified', None, [P, N]), ('require_full_candidate', None, [P]),
        ('set_segment_context', None, [P, S]), ('segment_context', S, [P]),
        ('candidate_count', Z, [P]), ('candidate', S, [P, Z]),
        ('candidate_consumed', Z, [P, Z]), ('candidate_info', N, [P, Z, C.POINTER(Info), Z]),
        ('page', Z, [P]), ('sentence_count', Z, [P]), ('max_input_bytes', Z, []),
    ]:
        fn = getattr(api, 'mpn_' + name)
        fn.restype, fn.argtypes = result, args
        globals()[name] = fn

    checked = selections = partials = transitions = 0
    randomizer = random.Random(0x4D494D4F)
    glyphs = [('你', '你'), ('我', '我'), ('好', '好'), ('是', '是'), ('嗎', '吗'), ('們', '们')]
    for mode, spellings, tones in [
        ('pinyin', ['ni3', 'wo3', 'hao3', 'shi4', 'ma1', 'men5'], '12345'),
        ('zhuyin', ['su3', 'ji3', 'cl3', 'g4', 'a8 ', 'ap7'], ' 6347'),
    ]:
        rows = []
        words = {(i,) for i in range(len(glyphs))}
        words.update(tuple(randomizer.randrange(6) for _ in range(randomizer.randrange(2, 4))) for _ in range(90))
        for ordinal, word in enumerate(sorted(words)):
            reading = ''.join(spellings[i] for i in word)
            bare = ''.join(c for c in reading if c not in tones)
            rows.append((bare, reading, ''.join(glyphs[i][0] for i in word), ''.join(glyphs[i][1] for i in word),
                         10000 - ordinal, 9000 + ordinal))
        rows.sort(key=lambda row: (row[0], -row[4]))
        data = (f'MEOWPUUT-{mode.upper()}-2\n' + ''.join('\t'.join(map(str, row)) + '\n' for row in rows)).encode()
        lexicon = lexicon_create(data, len(data))
        assert lexicon
        for hans in (0, 1):
            for full in (False, True):
                session = session_create(lexicon)
                set_simplified(session, hans)
                if full:
                    require_full_candidate(session)

                def snapshot():
                    global checked
                    raw = input(session).decode()
                    pending = committed(session).decode()
                    assert len(raw) <= max_input_bytes()
                    assert not raw or raw[0] not in tones + "'", (mode, raw)
                    result = []
                    for i in range(candidate_count(session)):
                        text = candidate(session, i).decode()
                        consumed = candidate_consumed(session, i)
                        info = Info()
                        assert candidate_info(session, i, C.byref(info), C.sizeof(info))
                        assert text and 0 < consumed <= len(raw), (mode, raw, text, consumed)
                        assert info.consumed == info.remaining == consumed
                        assert consumed == len(raw) or raw[consumed] not in tones + "'", (mode, raw, text, consumed)
                        result.append((text, consumed))
                    assert len(result) == len(set(result)), (mode, raw, result)
                    assert page(session) == 0 if not result else page(session) * 7 < len(result)
                    assert input(session).decode() == raw and committed(session).decode() == pending
                    checked += 1
                    return raw, pending, result

                def select(index):
                    global selections, partials, transitions
                    raw, pending, choices = snapshot()
                    displayed, consumed = choices[index]
                    expected = displayed
                    if full and index < sentence_count(session) and consumed < len(raw):
                        suffix = raw[consumed:]
                        assert displayed.endswith(suffix)
                        expected = displayed[:-len(suffix)]
                    assert choose(session, index)
                    assert input(session).decode() == raw[consumed:]
                    assert committed(session).decode() == pending + expected
                    assert committed(session).decode() == pending + expected # reads do not drain
                    selections += 1
                    partials += consumed < len(raw)
                    transitions += 1
                    snapshot()

                def type_keys(raw):
                    global transitions
                    for byte in raw.encode():
                        assert key(session, byte), (mode, raw, chr(byte))
                        transitions += 1
                        snapshot()

                try:
                    # All six syllables, wrong explicit tones, and arbitrary
                    # unfinished suffixes: no conversion may orphan the tone.
                    for spelling in spellings:
                        for wrong in tones:
                            if wrong == spelling[-1]:
                                continue
                            for suffix in ('x', 'z'):
                                clear(session)
                                type_keys(spelling[:-1] + wrong + suffix)

                    clear(session)
                    for round_number in range(32):
                        # Compose from many generated phrases, never prescribe
                        # which plausible text the decoder should prefer.
                        syllables = [randomizer.randrange(6) for _ in range(randomizer.randrange(4, 12))]
                        tokens = []
                        for i in syllables:
                            style = randomizer.randrange(3)
                            tokens.append(spellings[i] if style == 0 else spellings[i][:-1] + ("'" if style == 1 else ""))
                        type_keys(''.join(tokens))
                        raw, pending, choices = snapshot()
                        prefix = [i for i, (_, consumed) in enumerate(choices) if consumed < len(raw)]
                        if prefix:
                            select(randomizer.choice(prefix))
                        for _ in range(7):
                            raw, pending, choices = snapshot()
                            action = randomizer.randrange(7)
                            if action == 0 and choices:
                                select(randomizer.randrange(len(choices)))
                            elif action == 1:
                                assert bool(key(session, 0xff08)) == bool(raw)
                                assert input(session).decode() == raw[:-1]
                                assert committed(session).decode() == pending
                            elif action == 2:
                                set_simplified(session, randomizer.randrange(2))
                                assert input(session).decode() == raw and committed(session).decode() == pending
                            elif action == 3:
                                set_segment_context(session, randomizer.choice(['今天', '你好', '', '🙂']).encode())
                                assert input(session).decode() == raw and committed(session).decode() == pending
                            elif action == 4:
                                before = snapshot()
                                assert not choose(session, len(choices) + 99)
                                assert snapshot() == before
                            elif action == 5:
                                before = snapshot()
                                assert not key(session, ord('@'))
                                assert snapshot() == before
                            else:
                                key(session, 0xff56)
                                key(session, 0xff55)
                                assert input(session).decode() == raw and committed(session).decode() == pending
                            transitions += 1
                            snapshot()
                        # Escape discards only active reading, never an earlier
                        # selected commit; draining twice cannot repeat it.
                        pending = committed(session)
                        key(session, 0xff1b)
                        assert not input(session) and committed(session) == pending
                        clear_commit(session)
                        assert not committed(session)
                        clear_commit(session)
                        assert not committed(session)
                        snapshot()

                    # Rollover, next reading, pending output, backspace and
                    # setting changes remain independent at the capacity bound.
                    clear(session)
                    type_keys(spellings[0])
                    select(0)
                    pending = committed(session)
                    type_keys('z' * max_input_bytes())
                    visible = preedit(session)
                    type_keys('x')
                    assert input(session) == b'x' and committed(session) == pending + visible
                    set_simplified(session, 1 - hans)
                    assert input(session) == b'x' and committed(session) == pending + visible
                    assert key(session, 0xff08)
                    assert not input(session) and committed(session) == pending + visible
                    commit(session)
                    assert committed(session) == pending + visible
                    snapshot()
                finally:
                    session_destroy(session)
        lexicon_destroy(lexicon)
    assert partials >= 100
    print(f'PASS: {transitions} deterministic mutations, {checked} state checks, '
          f'{selections} selections ({partials} partial); both scripts/modes and full-candidate presentation')
