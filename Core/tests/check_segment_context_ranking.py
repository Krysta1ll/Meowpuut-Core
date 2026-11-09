#!/usr/bin/env python3
"""Check contextual phrase ranking and exact suffix ownership in both modes."""
import ctypes
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'

with tempfile.TemporaryDirectory(prefix='meowpuut-context-') as temporary:
    library = Path(temporary) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags,
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    pointer, text, size, integer = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int

    def bind(name, result, *arguments):
        function = getattr(core, name)
        function.restype, function.argtypes = result, arguments
        return function

    make_lexicon = bind('mpn_lexicon_create', pointer, text, size)
    destroy_lexicon = bind('mpn_lexicon_destroy', None, pointer)
    create = bind('mpn_session_create', pointer, pointer)
    destroy = bind('mpn_session_destroy', None, pointer)
    clear = bind('mpn_clear', None, pointer)
    set_context = bind('mpn_set_segment_context', None, pointer, text)
    get_context = bind('mpn_segment_context', text, pointer)
    set_hans = bind('mpn_set_simplified', None, pointer, integer)
    key = bind('mpn_key', integer, pointer, ctypes.c_int32)
    count = bind('mpn_candidate_count', size, pointer)
    candidate = bind('mpn_candidate', text, pointer, size)
    consumed = bind('mpn_candidate_consumed', size, pointer, size)
    choose = bind('mpn_choose', integer, pointer, size)
    get_input = bind('mpn_input', text, pointer)
    get_commit = bind('mpn_committed', text, pointer)

    cases = {
        'pinyin': [('日本', 'shengyou', '聲優', '声优'),
                   ('上', 'ketang', '嗑糖', '嗑糖'),
                   ('換', 'xinbao', '新包', '新包'),
                   ('我喜', 'huan', '歡', '欢')],
        'zhuyin': [('日本', 'g/ u. ', '聲優', '声优'),
                   ('上', 'dk w;6', '嗑糖', '嗑糖'),
                   ('換', 'vup 1l ', '新包', '新包'),
                   ('我喜', 'cj0 ', '歡', '欢')],
    }
    for mode, examples in cases.items():
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = make_lexicon(data, len(data))
        assert lexicon, mode
        session = create(lexicon)
        assert session, mode

        def enter(raw):
            for char in raw.encode('ascii'):
                assert key(session, char), (mode, raw, char)

        def visible():
            return [candidate(session, i).decode() for i in range(count(session))]

        try:
            for simplified in (0, 1):
                set_hans(session, simplified)
                for prefix, raw, traditional, simplified_word in examples:
                    expected = simplified_word if simplified else traditional
                    clear(session)
                    set_context(session, prefix.encode())
                    enter(raw)
                    assert visible()[0] == expected, (mode, prefix, raw, visible()[:7])
                    assert consumed(session, 0) == len(raw), (mode, raw, consumed(session, 0))
                    assert choose(session, 0)
                    assert get_commit(session).decode() == expected
                    assert not get_input(session) and not get_context(session)

                # Context must not survive cancel/clear and alter a later input.
                clear(session)
                raw = examples[0][1]
                enter(raw)
                baseline = visible()
                clear(session)
                set_context(session, '日本'.encode())
                enter(raw)
                clear(session)
                assert not get_context(session)
                enter(raw)
                assert visible() == baseline, (mode, 'context leaked after clear')

                # Single-character polyphones retain their earlier ordering:
                # `shui` after 部小 is not the shuo reading in 小說.
                if mode == 'pinyin':
                    clear(session)
                    set_context(session, '部小'.encode())
                    enter('shui')
                    assert visible()[0] != '說', visible()[:7]

            if mode == 'pinyin':
                # A phrase candidate may be selected without consuming the
                # unfinished next syllable.
                clear(session)
                set_hans(session, 0)
                set_context(session, '日本'.encode())
                enter('shengyoun')
                matches = visible()
                index = matches.index('聲優')
                assert consumed(session, index) == len('shengyou')
                assert choose(session, index)
                assert get_commit(session).decode() == '聲優' and get_input(session) == b'n'
        finally:
            destroy(session)
            destroy_lexicon(lexicon)

print('PASS: phrase context ranks, scripts, tones, suffixes and session isolation')
