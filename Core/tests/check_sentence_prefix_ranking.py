#!/usr/bin/env python3
"""Keep longer decoded prefixes and consumption boundaries during re-ranking."""
import ctypes as C
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
REPORT = '--report' in sys.argv

with tempfile.TemporaryDirectory(prefix='meowpuut-prefix-ranking-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC', '-I' + str(ROOT / 'Core'),
                    os.environ.get('MEOWPUUT_PREFIX_CORE', str(ROOT / 'Core/MeowpuutCore.cpp')),
                    '-o', str(library)], check=True)
    api = C.CDLL(str(library))
    P, S, N, Z = C.c_void_p, C.c_char_p, C.c_int, C.c_size_t
    for name, result, args in [
        ('lexicon_create', P, [S, Z]), ('lexicon_destroy', None, [P]),
        ('session_create', P, [P]), ('session_destroy', None, [P]),
        ('key', N, [P, N]), ('choose', N, [P, Z]), ('clear', None, [P]),
        ('set_simplified', None, [P, N]), ('set_segment_context', None, [P, S]),
        ('input', S, [P]), ('committed', S, [P]),
        ('candidate_count', Z, [P]), ('candidate', S, [P, Z]), ('candidate_consumed', Z, [P, Z]),
    ]:
        fn = getattr(api, 'mpn_' + name)
        fn.restype, fn.argtypes = result, args
        globals()[name] = fn

    def dictionary(mode, rows, context=()):
        rows = sorted(rows, key=lambda row: (row[0], -row[4]))
        text = f'MEOWPUUT-{mode.upper()}-2\n' + ''.join('\t'.join(map(str, row)) + '\n' for row in rows)
        if context:
            text += '# context\n' + ''.join(f'{word}\t{weight}\t{weight}\n' for word, weight in sorted(context))
        data = text.encode()
        owner = lexicon_create(data, len(data))
        assert owner
        return owner

    def row(bare, reading, text, weight=10000, hans=None):
        return bare, reading, text, hans or text, weight, weight

    def enter(session, raw):
        clear(session)
        for byte in raw.encode():
            assert key(session, byte), (raw, chr(byte))

    def choices(session):
        return [(candidate(session, i).decode(), candidate_consumed(session, i))
                for i in range(candidate_count(session))]

    longer_hits = longer_total = ordered_hits = ordered_total = selections = 0
    earlier_hits = earlier_total = real_hits = real_total = 0
    # The prefix is a recorded word followed by a complete independently
    # decoded syllable. The unfinished next syllable is not part of either.
    for head, head_reading, head_text in [('woxiang', 'wo3xiang3', '我想'), ('women', 'wo3men5', '我們')]:
        rows = [row(head, head_reading, head_text, hans=head_text.replace('們', '们')),
                row('wo', 'wo3', '我', 8000), row('xiang', 'xiang3', '想', 8000),
                row('men', 'men5', '們', 8000, '们'), row('ji', 'ji1', '機', 8000, '机'),
                row('qu', 'qu4', '去'), row('he', 'he1', '喝'), row('zu', 'zu1', '租')]
        owner = dictionary('pinyin', rows)
        session = session_create(owner)
        try:
            for hans in (0, 1):
                set_simplified(session, hans)
                for bare, output in [('qu', '去'), ('he', '喝'), ('zu', '租')]:
                    for spelled_head in (head, head.replace('wo', "wo'", 1)):
                        raw = spelled_head + bare + 'j'
                        expected = head_text.replace('們', '们') if hans else head_text
                        expected += output
                        enter(session, raw)
                        actual = choices(session)
                        target = expected, len(raw) - 1
                        found = target in actual[:7]
                        longer_hits += found
                        longer_total += 1
                        if not REPORT:
                            assert found, (raw, actual[:7], target)
                            assert choose(session, actual.index(target))
                            assert committed(session).decode() == expected and input(session) == b'j'
                            assert key(session, ord('i'))
                            following = '机' if hans else '機'
                            suffix_choices = choices(session)
                            assert (following, 2) in suffix_choices
                            assert choose(session, suffix_choices.index((following, 2)))
                            assert committed(session).decode() == expected + following and not input(session)
                            # Completing the previous syllable without the new
                            # initial must still prefer the complete input.
                            enter(session, raw[:-1])
                            assert choices(session)[0] == (expected, len(raw) - 1)
                            selections += 2
        finally:
            session_destroy(session)
            lexicon_destroy(owner)

    # The head is deliberately absent as a whole dictionary word: it must
    # still be selectable as an earlier split of the longer decoded prefix.
    rows = [row('wo', 'wo3', '我'), row('xiang', 'xiang3', '想'), row('men', 'men5', '們', hans='们'),
            row('qu', 'qu4', '去'), row('he', 'he1', '喝'), row('zu', 'zu1', '租'), row('ji', 'ji1', '機', hans='机')]
    owner = dictionary('pinyin', rows)
    session = session_create(owner)
    try:
        for hans in (0, 1):
            set_simplified(session, hans)
            for head, output in [('woxiang', '我想'), ('women', '我们' if hans else '我們')]:
                for tail in ('qu', 'he', 'zu'):
                    for spelled_head in (head, head.replace('wo', "wo'", 1)):
                        raw = spelled_head + tail + 'j'
                        enter(session, raw)
                        actual = choices(session)
                        target = output, len(spelled_head)
                        found = target in actual[:7]
                        earlier_hits += found
                        earlier_total += 1
                        if not REPORT:
                            assert found, (raw, actual[:7], target)
                            assert actual[0][1] == len(raw) - 1, (raw, actual[:7])
                            assert choose(session, actual.index(target))
                            assert input(session).decode() == tail + 'j' and committed(session).decode() == output
                            selections += 1
    finally:
        session_destroy(session)
        lexicon_destroy(owner)

    # These reviewed real-input traces are regression cases, not an unseen
    # accuracy sample. Keep their earlier useful segment selectable while the
    # next multi-syllable word is still incomplete.
    data = (ROOT / '.build/Meowpuut.app/Contents/Resources/native-pinyin.tsv').read_bytes()
    owner = lexicon_create(data, len(data))
    assert owner
    session = session_create(owner)
    try:
        for hans in (0, 1):
            set_simplified(session, hans)
            for raw, head, text in [('wobayaos', 'woba', '我把'), ('wobayaosh', 'woba', '我把'),
                                    ('qingbachuangh', 'qingba', '请把' if hans else '請把'),
                                    ('bagangcaidewenj', 'bagangcaide', '把刚才的' if hans else '把剛才的')]:
                enter(session, raw)
                actual = choices(session)
                target = text, len(head)
                found = target in actual[:7]
                real_hits += found
                real_total += 1
                if not REPORT:
                    assert found, (raw, actual[:7], target)
                    assert choose(session, actual.index(target))
                    assert input(session).decode() == raw[len(head):] and committed(session).decode() == text
                    selections += 1
    finally:
        session_destroy(session)
        lexicon_destroy(owner)

    # A strong previous-context match must not move a shorter selection ahead
    # of the complete paths already present. High-scoring suffix alternatives
    # intentionally prune that short prefix from the complete sentence beam.
    for mode, head_bare, head_reading, tail_bare, tail_reading, raw in [
        ('pinyin', 'nihao', 'ni3hao3', 'wo', 'wo3', 'nihaowo'),
        ('zhuyin', 'sucl', 'su3cl3', 'ji', 'ji3', 'su3cl3ji3'),
    ]:
        rows = [row(head_bare, head_reading, '同' + char, 10000 - i)
                for i, char in enumerate('甲乙丙丁戊己')]
        tails = '我她他它祂'
        rows += [row(tail_bare, tail_reading, char, 10000 - i) for i, char in enumerate(tails)]
        if mode == 'pinyin':
            rows += [row('ni', 'ni3', '你', 100), row('hao', 'hao3', '好', 100)]
        context = [(char + tail, 10000) for char in '乙丙丁戊己' for tail in tails]
        context += [('前同甲', 10000), ('後同丙', 9000)]
        owner = dictionary(mode, rows, context)
        session = session_create(owner)
        try:
            for hans in (0, 1):
                set_simplified(session, hans)
                enter(session, raw)
                baseline = choices(session)
                assert baseline[0][1] == len(raw)
                set_segment_context(session, '前'.encode())
                actual = choices(session)
                complete_first = actual[0][1] == len(raw)
                ordered_hits += complete_first
                ordered_total += 1
                if not REPORT:
                    assert complete_first, (mode, baseline[:7], actual[:7])
                    assert [n for _, n in actual[:64]] == [n for _, n in baseline[:64]]
                    # Context still improves homophone order within the same
                    # consumed span, rather than disabling contextual ranking.
                    set_segment_context(session, '後'.encode())
                    contextual = choices(session)
                    assert contextual[0][0].startswith('同丙'), (mode, contextual[:7])
                    set_segment_context(session, '前'.encode())
                    prefix = next(i for i, item in enumerate(actual) if item[0] == '同甲' and item[1] < len(raw))
                    consumed = actual[prefix][1]
                    assert choose(session, prefix)
                    assert input(session).decode() == raw[consumed:] and committed(session).decode() == '同甲'
                    selections += 1
        finally:
            session_destroy(session)
            lexicon_destroy(owner)
        # An exact dictionary entry keeps its whole-input priority even when
        # a shorter alternative has stronger preceding-context evidence.
        owner = dictionary(mode, rows + [row(head_bare + tail_bare, head_reading + tail_reading,
                                             '完整詞', 100, '完整词')], context)
        session = session_create(owner)
        try:
            for hans in (0, 1):
                set_simplified(session, hans)
                enter(session, raw)
                set_segment_context(session, '前'.encode())
                assert choices(session)[0] == ('完整词' if hans else '完整詞', len(raw))
        finally:
            session_destroy(session)
            lexicon_destroy(owner)
    print(f'PREFIX: {longer_hits}/{longer_total} longer prefixes in first page; '
          f'CONTEXT: {ordered_hits}/{ordered_total} complete-path priority; '
          f'EARLIER: {earlier_hits}/{earlier_total}; reviewed traces {real_hits}/{real_total}; '
          f'{selections} suffix-preserving selections')
