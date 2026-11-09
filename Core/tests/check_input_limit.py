#!/usr/bin/env python3
"""Verify a full composition never silently loses the next input key."""
import ctypes
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='meowpuut-limit-') as tmp:
    lib = Path(tmp) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    shared = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *shared,
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(lib)], check=True)
    core = ctypes.CDLL(str(lib))
    ptr, string, number, size = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_size_t
    for name, args, result in [
        ('mpn_lexicon_create', [string, size], ptr),
        ('mpn_lexicon_destroy', [ptr], None),
        ('mpn_session_create', [ptr], ptr),
        ('mpn_session_destroy', [ptr], None),
        ('mpn_max_input_bytes', [], size),
        ('mpn_key', [ptr, ctypes.c_int32], number),
        ('mpn_choose', [ptr, size], number),
        ('mpn_commit', [ptr], None),
        ('mpn_candidate_count', [ptr], size),
        ('mpn_candidate', [ptr, size], string),
        ('mpn_candidate_consumed', [ptr, size], size),
        ('mpn_input', [ptr], string),
        ('mpn_preedit', [ptr], string),
        ('mpn_committed', [ptr], string),
        ('mpn_clear', [ptr], None),
        ('mpn_clear_commit', [ptr], None),
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result

    limit = core.mpn_max_input_bytes()
    assert limit == 256
    dictionaries = [
        ('pinyin', 'MEOWPUUT-PINYIN-1\na\ta1\t啊\t啊\t10000\n', 'a', '啊', 'b'),
        ('zhuyin', 'MEOWPUUT-ZHUYIN-1\nj\tj3\t我\t我\t10000\n', 'j', '我', 'q'),
    ]
    for mode, dictionary, letter, first, following in dictionaries:
        data = dictionary.encode()
        lexicon = core.mpn_lexicon_create(data, len(data))
        assert lexicon, mode
        session = core.mpn_session_create(lexicon)
        assert session
        try:
            # A host can receive several edits before it drains the commit
            # buffer. Subsequent selections, literal commits and cancellation
            # must not overwrite text that was already accepted.
            core.mpn_clear(session)
            for _ in range(2):
                assert core.mpn_key(session, ord(letter))
                assert core.mpn_choose(session, 0)
            assert core.mpn_committed(session).decode() == first * 2
            core.mpn_commit(session)  # Empty composition is a no-op.
            assert core.mpn_committed(session).decode() == first * 2
            assert core.mpn_key(session, ord(following))
            assert core.mpn_candidate_count(session) == 0
            core.mpn_commit(session)
            literal = following if mode == 'pinyin' else 'ㄆ'
            assert core.mpn_committed(session).decode() == first * 2 + literal
            assert core.mpn_key(session, ord(letter))
            assert core.mpn_key(session, 0xff1b)
            assert core.mpn_committed(session).decode() == first * 2 + literal
            assert not core.mpn_input(session)
            core.mpn_clear_commit(session)

            def fill(pending=False):
                core.mpn_clear(session)
                if pending:
                    assert core.mpn_key(session, ord(letter))
                    assert core.mpn_candidate_count(session) > 0
                    assert core.mpn_candidate(session, 0).decode() == first
                    assert core.mpn_choose(session, 0)
                    assert core.mpn_committed(session).decode() == first
                for _ in range(limit):
                    assert core.mpn_key(session, ord(letter))
                assert core.mpn_input(session).decode() == letter * limit
                return core.mpn_preedit(session).decode()

            # The old committed text has not yet been read by the host.
            reading = fill(pending=True)
            assert core.mpn_key(session, ord(following))
            assert core.mpn_committed(session).decode() == first + reading
            assert core.mpn_input(session).decode() == following
            core.mpn_clear_commit(session)

            # Unsupported keys must not end or mutate the existing reading.
            reading = fill()
            assert not core.mpn_key(session, ord('@'))
            assert core.mpn_input(session).decode() == letter * limit
            assert core.mpn_preedit(session).decode() == reading
            assert not core.mpn_committed(session)

            # The tone belongs to the old syllable; it must not be dropped or
            # start a new composition with a standalone tone.
            reading = fill(pending=True)
            tone, rendered = ('4', '4') if mode == 'pinyin' else ('3', 'ˇ')
            assert core.mpn_key(session, ord(tone))
            assert core.mpn_committed(session).decode() == first + reading + rendered
            assert not core.mpn_input(session)
            core.mpn_clear_commit(session)

            # A first-tone space in Zhuyin is a mark; a Pinyin space remains a
            # literal separator. Neither may silently disappear at the limit.
            reading = fill(pending=True)
            assert core.mpn_key(session, ord(' '))
            assert core.mpn_committed(session).decode() == first + reading + (' ' if mode == 'pinyin' else 'ˉ')
            assert not core.mpn_input(session)

            # Apostrophe remains a separator and stays with the old reading.
            reading = fill()
            assert core.mpn_key(session, ord("'"))
            assert core.mpn_committed(session).decode() == reading + ("'" if mode == 'pinyin' else ' ')
            assert not core.mpn_input(session)

            # Repeated tones retain the pre-existing behavior: rejected in
            # Pinyin; ignored in Zhuyin. Neither mutates a full composition.
            core.mpn_clear(session)
            for _ in range(limit - 1):
                assert core.mpn_key(session, ord(letter))
            last_tone = '1' if mode == 'pinyin' else '3'
            assert core.mpn_key(session, ord(last_tone))
            raw = letter * (limit - 1) + last_tone
            before = core.mpn_preedit(session).decode()
            repeat = core.mpn_key(session, ord('4'))
            assert bool(repeat) == (mode == 'zhuyin')
            assert core.mpn_input(session).decode() == raw
            assert core.mpn_preedit(session).decode() == before
            assert not core.mpn_committed(session)

            # In Zhuyin, space after a tone confirms the current candidate as
            # before; the chosen prefix's untouched keys must remain.
            if mode == 'zhuyin':
                assert core.mpn_candidate_count(session)
                selected = core.mpn_candidate(session, 0).decode()
                consumed = core.mpn_candidate_consumed(session, 0)
                assert core.mpn_key(session, ord(' '))
                assert core.mpn_committed(session).decode() == selected
                assert core.mpn_input(session).decode() == raw[consumed:]
        finally:
            core.mpn_session_destroy(session)
            core.mpn_lexicon_destroy(lexicon)
    print('PASS: full Pinyin and Zhuyin readings, pending commits, new keys, tones and spaces survive the limit')
