#!/usr/bin/env python3
"""Verify long-pinyin recovery preserves the exact unconverted suffix."""
import ctypes as C
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
resources = root / '.build/Meowpuut.app/Contents/Resources'
data = (resources / 'native-pinyin.tsv').read_bytes()
with tempfile.TemporaryDirectory(prefix='meowpuut-prefix-recovery-') as tmp:
    library = Path(tmp) / 'core.dylib'
    subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC',
                    str(root / 'Core/MeowpuutCore.cpp'), '-o', str(library)], check=True)
    lib = C.CDLL(str(library))
    for name, result, args in [
        ('lexicon_create', C.c_void_p, [C.c_char_p, C.c_size_t]),
        ('session_create', C.c_void_p, [C.c_void_p]),
        ('set_simplified', None, [C.c_void_p, C.c_int]),
        ('key', C.c_int, [C.c_void_p, C.c_int]),
        ('candidate_count', C.c_size_t, [C.c_void_p]),
        ('candidate', C.c_char_p, [C.c_void_p, C.c_size_t]),
        ('candidate_consumed', C.c_size_t, [C.c_void_p, C.c_size_t]),
        ('choose', C.c_int, [C.c_void_p, C.c_size_t]),
        ('input', C.c_char_p, [C.c_void_p]),
        ('committed', C.c_char_p, [C.c_void_p]),
        ('session_destroy', None, [C.c_void_p]),
        ('lexicon_destroy', None, [C.c_void_p]),
    ]:
        fn = getattr(lib, 'mpn_' + name)
        fn.restype, fn.argtypes = result, args
    lexicon = lib.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = lib.mpn_session_create(lexicon)
    assert session
    lib.mpn_set_simplified(session, 1)
    reading = b'kuaididaoleqingbangwonayixia'
    for key in reading:
        assert lib.mpn_key(session, key)
    matches = [i for i in range(lib.mpn_candidate_count(session))
               if lib.mpn_candidate(session, i).decode() == '快递'
               and lib.mpn_candidate_consumed(session, i) == 6]
    assert matches, 'A useful early phrase must remain selectable after a long input'
    assert lib.mpn_choose(session, matches[0])
    assert lib.mpn_committed(session).decode() == '快递'
    assert lib.mpn_input(session) == reading[6:]
    lib.mpn_session_destroy(session)
    session = lib.mpn_session_create(lexicon)
    assert session
    lib.mpn_set_simplified(session, 1)
    for key in b'nihao':
        assert lib.mpn_key(session, key)
    matches = [i for i in range(lib.mpn_candidate_count(session))
               if lib.mpn_candidate(session, i).decode() == '你'
               and lib.mpn_candidate_consumed(session, i) == 2]
    assert matches, 'A two-syllable input must expose first-character candidates'
    assert lib.mpn_choose(session, matches[0])
    assert lib.mpn_committed(session).decode() == '你'
    assert lib.mpn_input(session) == b'hao'
    lib.mpn_session_destroy(session)
    lib.mpn_lexicon_destroy(lexicon)
    data = (resources / 'native-zhuyin.tsv').read_bytes()
    lexicon = lib.mpn_lexicon_create(data, len(data))
    assert lexicon
    session = lib.mpn_session_create(lexicon)
    assert session
    for key in b'su3cl3':
        assert lib.mpn_key(session, key)
    matches = [i for i in range(lib.mpn_candidate_count(session))
               if lib.mpn_candidate(session, i).decode() == '你'
               and lib.mpn_candidate_consumed(session, i) == 3]
    assert matches, 'A two-syllable Zhuyin input must expose first-character candidates'
    assert lib.mpn_choose(session, matches[0])
    assert lib.mpn_committed(session).decode() == '你'
    assert lib.mpn_input(session) == b'cl3'
    lib.mpn_session_destroy(session)
    lib.mpn_lexicon_destroy(lexicon)
print('PASS: Pinyin and Zhuyin retain selectable prefixes and their unconverted suffixes')
