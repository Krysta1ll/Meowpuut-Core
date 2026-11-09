#!/usr/bin/env python3
"""Check dual-score generation without depending on any platform adapter."""
import importlib.util
import json
import math
from pathlib import Path
import sqlite3
import sys

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
sys.path.insert(0, str(root / 'scripts'))
spec = importlib.util.spec_from_file_location('mimo_compiler', root / 'scripts/compile-native-lexicon.py')
compiler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compiler)
source = json.loads((root / 'DataSources/PinyinPro/complete.json').read_text(encoding='utf-8'))

def score(word, toned):
    reading, frequency = source[word][:2]
    assert ''.join(compiler.numbered_pinyin(syllable) for syllable in reading.split()) == toned
    return max(1, round(700 * math.log1p(max(0, float(frequency)) * 1e12)))

with sqlite3.connect(resources / 'language-evidence.sqlite') as db:
    for hans, toned, spellings in [
        ('十周', 'shi2zhou1', ('十周', '十週')),
        ('靠你了', 'kao4ni3le5', ('靠你了', '靠妳了')),
    ]:
        rows = db.execute('SELECT hant,weight,hans_weight FROM lexicon WHERE hans=? AND toned=?',
                          (hans, toned)).fetchall()
        by_hant = {hant: (weight, hans_weight) for hant, weight, hans_weight in rows}
        assert all(hant in by_hant for hant in spellings), (hans, rows)
        assert all(by_hant[hant][1] == score(hans, toned) for hant in spellings), (hans, rows)
        # The traditional priors remain distinct; neither is replaced by the
        # simplified source frequency to achieve this result.
        assert len({by_hant[hant][0] for hant in spellings}) > 1, (hans, rows)
    for hant, hans, toned, weight in [('微信', '微信', 'wei1xin4', 9000),
                                     ('你好', '你好', 'ni3hao3', 10000),
                                     ('妳好', '你好', 'ni3hao3', 1000)]:
        assert db.execute('SELECT weight,hans_weight FROM lexicon WHERE hant=? AND hans=? AND toned=?',
                          (hant, hans, toned)).fetchone() == (weight, weight)
    for hans, toned, spellings, edited in [
        ('学姐', 'xue2jie3', ('學姊', '學姐'), 6000),
        ('惊艳', 'jing1yan4', ('驚艷', '驚豔'), 4500),
    ]:
        original = score(hans, toned)
        rows = {hant: (weight, hans_weight) for hant, weight, hans_weight in
                db.execute('SELECT hant,weight,hans_weight FROM lexicon WHERE hans=? AND toned=?', (hans, toned))}
        assert all(hant in rows and rows[hant][1] == edited for hant in spellings), (hans, rows)
        assert original != edited and len({rows[hant][0] for hant in spellings}) > 1, (hans, rows)
    assert '阿宝' not in source
    assert db.execute("SELECT weight=hans_weight FROM lexicon WHERE hant='阿寶' AND hans='阿宝' AND toned='a1bao3'").fetchone() == (1,)
    plan = db.execute('EXPLAIN QUERY PLAN SELECT hans,hans_weight FROM lexicon WHERE reading=? ORDER BY hans_weight DESC LIMIT 2048',
                      ('xuejie',)).fetchall()
    assert any('lexicon_reading_hans_weight' in item[-1] for item in plan), plan

expected = {}
for mode in ('pinyin', 'zhuyin'):
    path = resources / f'native-{mode}.tsv'
    with path.open(encoding='utf-8') as stream:
        assert stream.readline() == f'MEOWPUUT-{mode.upper()}-2\n'
        previous_bare, previous_weight = '', None
        for line in stream:
            if line == '# context\n': break
            fields = line.rstrip('\n').split('\t')
            assert len(fields) == 6, (mode, fields)
            bare, reading, hant, hans, weight, hans_weight = fields
            weight, hans_weight = int(weight), int(hans_weight)
            assert bare >= previous_bare and (bare != previous_bare or previous_weight >= weight)
            previous_bare, previous_weight = bare, weight
            if (hant, hans) in {('學姐', '学姐'), ('學姊', '学姐'), ('略', '略')}:
                expected[(mode, bare, reading, hant, hans)] = (weight, hans_weight)
for hant in ('學姐', '學姊'):
    weights = expected[('pinyin', 'xuejie', 'xue2jie3', hant, '学姐')]
    assert weights[1] == 6000  # explicit simplified editorial priority
    assert expected[('pinyin', 'xj', 'x2j3', hant, '学姐')] == tuple(max(1, value // 8) for value in weights)
    phonetic = compiler.encode('xue2') + compiler.encode('jie3')
    bare = phonetic.translate(str.maketrans('', '', ' 6347'))
    assert expected[('zhuyin', bare, phonetic, hant, '学姐')] == weights
assert expected[('pinyin', 'lue', 'lue4', '略', '略')] == expected[('pinyin', 'lve', 'lve4', '略', '略')]
print('PASS: dual source weights, traditional priors, editorial overrides, fallback, tones and aliases')
