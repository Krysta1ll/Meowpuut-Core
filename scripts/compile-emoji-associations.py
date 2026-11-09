#!/usr/bin/env python3
"""Build exact Chinese emoji aliases from pinned Unicode/CLDR data."""
import json
import re
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / 'DataSources/CLDR'

def build():
    canonical = {}
    for line in (DATA / 'emoji-test.txt').read_text(encoding='utf-8').splitlines():
        m = re.match(r'^([0-9A-F ]+)\s*;\s*fully-qualified\s*#', line)
        if m:
            text = ''.join(chr(int(cp, 16)) for cp in m.group(1).split())
            canonical[text.replace('\ufe0f', '')] = text
    aliases = defaultdict(dict)
    matched = set()
    for name in ('zh.xml', 'zh_Hant.xml', 'zh-derived.xml', 'zh-Hant-derived.xml'):
        for node in ET.parse(DATA / name).iter('annotation'):
            emoji = canonical.get(node.get('cp', '').replace('\ufe0f', ''))
            if not emoji or not node.text or node.text == '↑↑↑':
                continue
            matched.add(emoji)
            priority = 0 if node.get('type') == 'tts' else 1
            terms = set()
            for part in node.text.split('|'):
                term = part.strip()
                terms.add(term)
                terms.update(piece.strip() for piece in re.split(r'[，,]', term))
                # Gendered and skin-tone variants remain discoverable under
                # their shared role, after the unmodified version.
                if term.startswith(('男', '女')) and len(term) > 2:
                    terms.add(term[1:])
            for term in terms:
                if not term or term == '↑↑↑' or len(term) > 32:
                    continue
                skin = any(0x1f3fb <= ord(c) <= 0x1f3ff for c in emoji)
                gender = '\u200d♀' in emoji or '\u200d♂' in emoji
                rank = (priority, skin, gender, len(emoji), emoji)
                old = aliases[term].get(emoji)
                if old is None or rank < old:
                    aliases[term][emoji] = rank
    # CLDR's black-flag labels are more specific than the core's flag terms.
    for term in ('旗幟', '旗帜'):
        aliases[term]['🏴'] = (2, False, False, 1, '🏴')
    result = {term: sorted(emojis, key=emojis.get) for term, emojis in sorted(aliases.items())}
    return result, len(canonical), len(matched)

if __name__ == '__main__':
    result, total, matched = build()
    output = ROOT / 'Resources/Lexicon/emoji-cldr.json'
    output.write_text(json.dumps(result, ensure_ascii=False, separators=(',', ':')) + '\n', encoding='utf-8')
    print(f'{len(result)} Chinese aliases; {matched}/{total} fully qualified emoji annotated')
