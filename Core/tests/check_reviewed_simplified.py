#!/usr/bin/env python3
"""Proposal regression; expects only the explicitly reviewed tuples to be filtered.

Usage: python3 Core/tests/check_reviewed_simplified.py [resources]
Checks the compiled SQLite and portable core, not platform rendering or user data.
"""
import ctypes
import importlib.util
import re
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0, str(root / 'scripts'))
spec = importlib.util.spec_from_file_location('mimo_compiler', root / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec); spec.loader.exec_module(compiler)
rules_path = root / 'Resources/Lexicon/reviewed-simplified-exclusions.tsv'
reviewed = []
for line in rules_path.read_text(encoding='utf-8').splitlines():
    if not line or line.startswith('#'):
        continue
    fields = line.split('|')
    assert len(fields) == 4, line
    reviewed.append(tuple(fields))
assert len(set(reviewed)) == len(reviewed), 'duplicate reviewed exclusion'

# These cases specifically protect the reported bug. Additional future rules
# may be reviewed separately and should add their own regression tests.
required = {
    ('為', 'wei2', '為', '为'), ('們', 'men2', '們', '们'),
    ('裡', 'li3', '裡', '里'), ('係', 'xi4', '係', '系'),
    ('對於', 'dui4yu2', '对於', '对于'), ('治療', 'zhi4liao2', '治療', '治疗'),
    ('至於', 'zhi4yu2', '至於', '至于'), ('有利於', 'you3li4yu2', '有利於', '有利于'),
    ('遊', 'you2', '遊', '游'), ('複', 'fu4', '複', '复'), ('麵', 'mian4', '麵', '面'),
    ('妳', 'ni3', '妳', '你'), ('妳好', 'ni3hao3', '妳好', '你好'),
}
assert required.issubset(set(reviewed)), 'reviewed coverage is missing'
items=[]
for hant,toned,bad,good in reviewed:
    sounds=re.findall(r'[a-zv]+[1-5]', toned)
    assert ''.join(sounds)==toned and len(hant)==len(good)==len(sounds), (hant,toned)
    items.append((hant,toned,bad,good,sounds))
# Protect valid context-sensitive forms; this does not require known-bad
# alternatives such as 干隆 or 具乐部 to remain in future dictionaries.
negative=[('著','zhu4','著'),('乾隆','qian2long2','乾隆'),
          ('紀錄','ji4lu4','记录'),('紀錄','ji4lu4','纪录'),('俱樂部','ju4le4bu4','俱乐部'),
          ('著名','zhu4ming2','著名'),('註冊','zhu4ce4','注册'),
          ('為','wei4','为'),('們','men5','们'),('係','xi5','系')]
keys = {(hant, toned, bad) for hant, toned, bad, _ in reviewed}
assert len(keys) == len(reviewed), 'same exact exclusion maps to multiple preferred forms'
for hant, toned, valid_hans in negative:
    assert (hant, toned, valid_hans) not in keys, ('valid context targeted', hant, toned, valid_hans)

# Exercise the generator in isolation. Missing reviewed records/alternatives
# must stop without applying an earlier rule or losing other native entries.
def check_rule_validation():
    original = compiler.authored
    def rules(entries):
        compiler.authored = lambda name: iter(enumerate(entries, 1))
    try:
        bad = ('為', '為', 'wei2')
        good = ('為', '为', 'wei2')
        untouched = {('為', '為', 'wei4'): 12, ('爲', '為', 'wei2'): 13,
                     ('為', '为', 'wei4'): 14}
        first = ['為', 'wei2', '為', '为']
        second = ['們', 'men2', '們', '们']
        for words, entries, reason in [
            ({bad: 1, **untouched}, [first], 'correct alternative missing'),
            ({good: 2, **untouched}, [first], 'source variant missing'),
            ({bad: 1, good: 2, ('們', '們', 'men2'): 3, **untouched},
             [first, second], 'correct alternative missing'),
            ({bad: 1, good: 2, ('們', '们', 'men2'): 3, **untouched},
             [first, second], 'source variant missing'),
        ]:
            before = words.copy()
            native = set(words)
            rules(entries)
            try:
                compiler.apply_reviewed_simplified_exclusions(words, native)
                raise AssertionError(('accepted invalid reviewed rule', reason))
            except ValueError as error:
                assert reason in str(error), (reason, str(error))
            assert words == before and native == set(before), ('partially applied reviewed rules', reason)
        words = {bad: 1, good: 2, **untouched}
        native = set(words)
        rules([first])
        assert compiler.apply_reviewed_simplified_exclusions(words, native) == 1
        assert bad not in words and bad not in native
        assert words == {good: 2, **untouched} and native == set(words)
        # A conflicting authored entry must stop the build before mutation;
        # otherwise canonical.update(reviewed_groups) could resurrect it.
        words = {bad: 1, good: 2, **untouched}
        before, native = words.copy(), set(words)
        try:
            compiler.apply_reviewed_simplified_exclusions(words, native, [bad])
            raise AssertionError('accepted conflicting authored entry')
        except ValueError as error:
            assert 'conflicts with reviewed overlay' in str(error)
        assert words == before and native == set(before)
    finally:
        compiler.authored = original

check_rule_validation()
with sqlite3.connect((resources / 'language-evidence.sqlite').resolve().as_uri()+'?mode=ro',uri=True) as db:
    for hant,toned,bad,good,_ in items:
        assert db.execute('SELECT 1 FROM lexicon WHERE hant=? AND toned=? AND hans=?',(hant,toned,good)).fetchone(), ('preferred missing',hant,toned,good)
        assert not db.execute('SELECT 1 FROM lexicon WHERE hant=? AND toned=? AND hans=?',(hant,toned,bad)).fetchone(), ('unwanted persists',hant,toned,bad)
    for hant,toned,hans in negative:
        assert db.execute('SELECT 1 FROM lexicon WHERE hant=? AND toned=? AND hans=?',(hant,toned,hans)).fetchone(), ('negative control changed',hant,toned,hans)

with tempfile.TemporaryDirectory(prefix='mimo-reviewed-tuples-') as tmp:
    lib=Path(tmp)/('core.dylib' if sys.platform=='darwin' else 'core.so')
    flags=['-dynamiclib'] if sys.platform=='darwin' else ['-shared','-fPIC']
    subprocess.run(['c++','-std=c++17','-O2',*flags,str(root/'Core/MeowpuutCore.cpp'),'-o',str(lib)],check=True)
    core=ctypes.CDLL(str(lib))
    for name,args,result in [
        ('mpn_lexicon_create',[ctypes.c_char_p,ctypes.c_size_t],ctypes.c_void_p),('mpn_lexicon_destroy',[ctypes.c_void_p],None),
        ('mpn_session_create',[ctypes.c_void_p],ctypes.c_void_p),('mpn_session_destroy',[ctypes.c_void_p],None),
        ('mpn_set_simplified',[ctypes.c_void_p,ctypes.c_int],None),('mpn_clear',[ctypes.c_void_p],None),
        ('mpn_key',[ctypes.c_void_p,ctypes.c_int32],ctypes.c_int),('mpn_candidate_count',[ctypes.c_void_p],ctypes.c_size_t),
        ('mpn_candidate',[ctypes.c_void_p,ctypes.c_size_t],ctypes.c_char_p),('mpn_choose',[ctypes.c_void_p,ctypes.c_size_t],ctypes.c_int),
        ('mpn_committed',[ctypes.c_void_p],ctypes.c_char_p)]:
        f=getattr(core,name); f.argtypes=args; f.restype=result
    count=0
    for mode in ('pinyin','zhuyin'):
        data=(resources/f'native-{mode}.tsv').read_bytes();lex=core.mpn_lexicon_create(data,len(data));assert lex
        session=core.mpn_session_create(lex);assert session
        try:
            for hant,toned,bad,good,sounds in items:
                # Explicit tone tests ensure another reading of the same Chinese character stays untouched.
                raw=toned if mode=='pinyin' else ''.join(compiler.encode(s) for s in sounds)
                for simplified,expected in [(0,hant),(1,good)]:
                    core.mpn_clear(session);core.mpn_set_simplified(session,simplified)
                    for key in raw:assert core.mpn_key(session,ord(key)),(mode,raw,key)
                    candidates=[core.mpn_candidate(session,i).decode() for i in range(core.mpn_candidate_count(session))]
                    assert expected in candidates,(mode,raw,simplified,expected,candidates[:20])
                    if simplified:
                        assert bad not in candidates,(mode,raw,bad,candidates[:30])
                        assert candidates.count(good)==1,(mode,raw,good)
                    assert core.mpn_choose(session,candidates.index(expected))
                    assert core.mpn_committed(session).decode()==expected
                    count+=1
        finally:
            core.mpn_session_destroy(session);core.mpn_lexicon_destroy(lex)
print(f'PASS: {len(items)} exact tuples; {count} pinyin/zhuyin, traditional/simplified selections; {len(negative)} ambiguity controls')
