#!/usr/bin/env python3
"""Check narrowly reviewed simplified priorities and traditional invariants."""
import ctypes
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0, str(root / 'scripts'))
spec = importlib.util.spec_from_file_location('mimo_compiler', root / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)

# A rule must name an existing word *and* the exact PinyinPro reading.  Check
# all entries before returning; conflicts cannot mutate input dictionaries.
words = {('學姐', '学姐', 'xue2 jie3'): 2414}
source = {('学姐', 'xue2 jie3'): 970}
original = compiler.authored
try:
    def rules(rows):
        compiler.authored = lambda name: iter(enumerate(rows, 1))
        return compiler.reviewed_simplified_priorities(words, source)
    assert rules([['学姐', 'xue2 jie3', '6000']]) == {('学姐', 'xue2 jie3'): 6000}
    for rows in ([['不存在', 'bu4 cun2 zai4', '6000']],
                 [['学姐', 'xue2 jie4', '6000']],
                 [['学姐', 'xue2 jie3', '6000'], ['学姐', 'xue2 jie3', '6500']],
                 [['学姐', 'xue2 jie3', '10001']]):
        try:
            rules(rows)
            raise AssertionError(('accepted invalid rules', rows))
        except ValueError:
            assert words == {('學姐', '学姐', 'xue2 jie3'): 2414} and source == {('学姐', 'xue2 jie3'): 970}
    compiler.authored = lambda name: iter([(1, ['学姐', 'xue2 jie3', '6000'])])
    try:
        compiler.reviewed_simplified_priorities(words, source, set(words))
        raise AssertionError('accepted conflicting curated overlay')
    except ValueError:
        assert words == {('學姐', '学姐', 'xue2 jie3'): 2414}
finally:
    compiler.authored = original

manifest = json.loads((resources / 'mimo-manifest.json').read_text(encoding='utf-8'))
record = manifest['simplified_editorial_priorities']
priority_data = (root / 'Resources/Lexicon/simplified-priorities.tsv').read_bytes().replace(b'\r\n', b'\n')
assert record == {'pairs': 2, 'sha256': hashlib.sha256(priority_data).hexdigest()}

with tempfile.TemporaryDirectory(prefix='meowpuut-editorial-') as directory:
    library = Path(directory) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
    flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
    subprocess.run(['c++', '-std=c++17', '-O2', *flags, str(root / 'Core/MeowpuutCore.cpp'),
                    '-o', str(library)], check=True)
    core = ctypes.CDLL(str(library))
    ptr, size = ctypes.c_void_p, ctypes.c_size_t
    for name, args, result in [
        ('mpn_lexicon_create', [ctypes.c_char_p, size], ptr), ('mpn_lexicon_destroy', [ptr], None),
        ('mpn_session_create', [ptr], ptr), ('mpn_session_destroy', [ptr], None),
        ('mpn_set_simplified', [ptr, ctypes.c_int], None), ('mpn_clear', [ptr], None),
        ('mpn_key', [ptr, ctypes.c_int32], ctypes.c_int), ('mpn_candidate_count', [ptr], size),
        ('mpn_candidate', [ptr, size], ctypes.c_char_p), ('mpn_choose', [ptr, size], ctypes.c_int),
        ('mpn_committed', [ptr], ctypes.c_char_p),
    ]:
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, result
    for mode in ('pinyin', 'zhuyin'):
        data = (resources / f'native-{mode}.tsv').read_bytes()
        lexicon = core.mpn_lexicon_create(data, len(data)); assert lexicon
        session = core.mpn_session_create(lexicon); assert session
        try:
            for sounds, preferred, peer, trad_first, trad_second in [
                (['xue2', 'jie3'], '学姐', '学界', '學姐', '學姊'),
                (['jing1', 'yan4'], '惊艳', '经验', '驚豔', '驚艷'),
            ]:
                keys = ''.join(s[:-1] for s in sounds) if mode == 'pinyin' else ''.join(compiler.encode(s)[:-1] for s in sounds)
                for simplified in (0, 1):
                    core.mpn_clear(session); core.mpn_set_simplified(session, simplified)
                    for key in keys: assert core.mpn_key(session, ord(key)), (mode, keys, key)
                    candidates = [core.mpn_candidate(session, i).decode() for i in range(core.mpn_candidate_count(session))]
                    if simplified:
                        assert preferred in candidates[:7], (mode, keys, candidates[:7])
                        if preferred == '学姐': assert candidates[0] == preferred, (mode, candidates[:7])
                        else: assert candidates[0] == peer and candidates.index(peer) < candidates.index(preferred), (mode, candidates[:7])
                        assert core.mpn_choose(session, candidates.index(preferred))
                        assert core.mpn_committed(session).decode() == preferred
                    else:
                        assert trad_first in candidates and trad_second in candidates, (mode, keys, candidates[:20])
                        assert candidates.index(trad_first) < candidates.index(trad_second), (mode, keys, candidates[:20])
        finally:
            core.mpn_session_destroy(session); core.mpn_lexicon_destroy(lexicon)
print('PASS: reviewed simplified candidates, exact rule validation and unchanged traditional variant order')
