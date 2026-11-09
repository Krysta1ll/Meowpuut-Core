#!/usr/bin/env python3
import importlib.util
import json
import sqlite3
from pathlib import Path

root = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('emoji_aliases', root / 'scripts/compile-emoji-associations.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
result, total, matched = module.build()
assert result == json.loads((root / 'Resources/Lexicon/emoji-cldr.json').read_text(encoding='utf-8'))
assert matched == total and total >= 3900
for word, symbol in [('苹果', '🍎'), ('蘋果', '🍎'), ('飞机', '✈️'), ('飛機', '✈️'), ('點頭', '🙂‍↕️'), ('吸血鬼', '🧛‍♀️')]:
    assert symbol in result[word], (word, symbol)
with sqlite3.connect(root / 'build/lexicon/language-evidence.sqlite') as db:
    words = {word for row in db.execute('select distinct hant,hans from lexicon') for word in row}
all_emoji = {emoji for values in result.values() for emoji in values}
covered = {emoji for word, values in result.items() if word in words for emoji in values}
assert covered == all_emoji, f'{len(all_emoji-covered)} emoji lack an existing Chinese lexicon alias'
print(f'PASS: {total} fully qualified emoji; {len(result)} aliases; all have a Chinese lexicon match')
