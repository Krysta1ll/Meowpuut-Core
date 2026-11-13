#!/usr/bin/env python3
"""Check generated Japanese readings, alternatives and duplicate entries."""
import json
import sys
import hashlib
from pathlib import Path
from japanese_readings import pinyin_kana

resources = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.build/lexicon')
readings = json.loads((resources / 'native-japanese.json').read_text())
for reading, entries in readings.items():
    pairs = [(item['text'], item['weight']) for item in entries]
    assert len({text for text, _ in pairs}) == len(pairs), f'duplicate {reading}: {pairs}'
    assert pairs == sorted(pairs, key=lambda item: (-item[1], item[0])), f'unsorted {reading}'
    assert all(text and isinstance(weight, int) and 0 < weight <= 10000 for text, weight in pairs), reading

for reading, expected in {
    'oshi': '推し', 'oshikatsu': '推し活', 'tuishihuo': '推し活', 'seiyuu': '声優', 'seiyu': '声優',
    'doujinshi': '同人誌', 'haishin': '配信', 'douga': '動画', 'kashi': '歌詞',
    'kashu': '歌手', 'onsei': '音声', 'utattemita': '歌ってみた', 'utaite': '歌い手',
    'sakuhin': '作品', 'monogatari': '物語', 'kyara': 'キャラ', 'kyarakutaa': 'キャラクター',
    'sutoorii': 'ストーリー', 'ningen': '人間', 'egao': '笑顔', 'yasashii': '優しい',
    'atsui': '暑い', 'samui': '寒い', 'issho': '一緒', 'minna': 'みんな',
    'zutto': 'ずっと', 'mata': 'また', 'tabun': '多分', 'mochiron': 'もちろん',
    'kitto': 'きっと', 'shiawase': '幸せ', 'taihen': '大変', 'kirakira': 'キラキラ',
    'dokidoki': 'ドキドキ', 'sukoshi': '少し', 'chigau': '違う', 'sou': 'そう',
    'bangumi': '番組', 'karaoke': 'カラオケ',
}.items():
    assert readings[reading][0]['text'] == expected, (reading, readings.get(reading))

for standard, alias, expected in [
    ('suki', 'haoki', '好き'), ('oshikatsu', 'tuishihuo', '推し活'),
    ('taberu', 'shiberu', '食べる'), ('nomu', 'yinmu', '飲む'),
    ('iku', 'xingku', '行く'), ('kawaii', 'keaii', '可愛い'),
    ('ongaku', 'yinyue', '音楽'), ('sanpo', 'sanbu', '散歩'),
]:
    romaji = next(item['weight'] for item in readings[standard] if item['text'] == expected)
    alternate = next(item['weight'] for item in readings[alias] if item['text'] == expected)
    assert romaji > alternate, (standard, alias, expected, romaji, alternate)
assert pinyin_kana('食べる', ['shi']) == 'shiberu'
assert pinyin_kana('食べちゃう', ['shi']) == 'shibechau'
assert pinyin_kana('推し活', ['tui', 'huo']) == 'tuishihuo'

root = Path(__file__).resolve().parent.parent
source = root / 'DataSources/mimo-jp'
manifest = json.loads((source / 'SOURCE.json').read_text())
assert manifest['name'] == 'mimo-jp'
# The generated snapshot and every notice file match the source record.
for name, expected in manifest['files'].items():
    assert hashlib.sha256((source / name).read_bytes()).hexdigest() == expected, name

# Mixed-input adapters register the first candidate only for each reading.
# Explicitly guard collisions so a newly merged row cannot silently shadow a
# reviewed word. The common 優しい takes precedence over 易しい intentionally.
reviewed_shadowed = {('yasashii', '易しい'): '優しい'}
observed_shadowed = {}
for line in (root / 'Resources/Lexicon/japanese-reviewed.tsv').read_text().splitlines():
    if not line or line.startswith('#'): continue
    _, word, standard, syllables, _ = line.split('|')
    alias = pinyin_kana(word, syllables.split())
    standard_entry = next(item for item in readings[standard] if item['text'] == word)
    alias_entry = next(item for item in readings[alias] if item['text'] == word)
    assert standard == alias or standard_entry['weight'] > alias_entry['weight'], (standard, alias, word)
    for reading in {standard, alias}:
        if readings[reading][0]['text'] != word:
            observed_shadowed[(reading, word)] = readings[reading][0]['text']
assert observed_shadowed == reviewed_shadowed, observed_shadowed

# The core registration pool is bounded; Swift and Windows register the same
# top entry per key, including the one reviewed two-character reading.
def core_eligible(reading, weight):
    return (reading.isascii() and reading.isalpha() and len(reading) <= 20
            and (len(reading) >= 3 or reading == 'au' and weight >= 4000)
            and (weight >= 4000 or weight == 3500))
eligible = {reading: entries[0] for reading, entries in readings.items()
            if entries and core_eligible(reading, entries[0]['weight'])}
assert len(eligible) <= 512, f'{len(eligible)} Japanese readings exceed the mixed-input registration pool'
assert eligible['au']['text'] == '会う'
assert not core_eligible('ai', 7000)  # A common Chinese syllable is not evidence of Japanese.
assert readings['yinyue'][0]['weight'] == 3500  # Pure-kanji alias is ambiguous.

# A compiled directory is self-contained only when these notices accompany it.
for dest, original in [('mimo-jp-BSD.txt', 'BSD.txt'),
                       ('mimo-jp-derived-BSD.txt', 'LICENSE.txt'),
                       ('mimo-jp-COPYING.txt', 'COPYING.txt'),
                       ('mimo-jp-SOURCE.json', 'SOURCE.json'),
                       ('mimo-jp-Mozc-BSD.txt', 'MOZC-LICENSE.txt'),
                       ('mimo-jp-Unicode-3.0.txt', 'UNICODE-LICENSE.txt')]:
    assert (resources / 'Licenses' / dest).read_bytes() == (source / original).read_bytes(), dest

tsv = [line.split('|') for line in (resources / 'native-japanese.tsv').read_text().splitlines()]
assert all(len(parts) == 3 for parts in tsv)
tsv_map = {}
for reading, word, weight in tsv:
    tsv_map.setdefault(reading, []).append({'text': word, 'weight': int(weight)})
assert tsv_map == readings, 'JSON and Windows TSV must have the same sorted entries'
print(f'PASS: {len(readings)} Japanese readings, sorted alternatives, no duplicates, reviewed words and aliases')
