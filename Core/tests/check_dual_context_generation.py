#!/usr/bin/env python3
"""Check every generated context score against the corresponding script prior."""
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[2]
resources = Path(sys.argv[1]) if len(sys.argv) > 1 else root / '.build/Meowpuut.app/Contents/Resources'
reviewed = {}
for line in (root / 'Resources/Lexicon/context.tsv').read_text(encoding='utf-8').splitlines():
    if not line or line.startswith('#'):
        continue
    phrase, priority = line.split('|')
    reviewed[phrase] = max(reviewed.get(phrase, 0), int(priority))

generated = []
for mode in ('pinyin', 'zhuyin'):
    expected, actual = {}, {}
    with (resources / f'native-{mode}.tsv').open(encoding='utf-8') as stream:
        assert stream.readline() == f'MEOWPUUT-{mode.upper()}-2\n'
        for line in stream:
            if line == '# context\n':
                break
            fields = line.rstrip('\n').split('\t')
            assert len(fields) == 6, fields
            _, _, hant, _, traditional, simplified = fields
            if len(hant) > 1:
                previous = expected.get(hant, (0, 0))
                expected[hant] = (max(previous[0], int(traditional)), max(previous[1], int(simplified)))
        else:
            raise AssertionError(f'{mode}: context section missing')
        for phrase, priority in reviewed.items():
            previous = expected.get(phrase, (0, 0))
            expected[phrase] = (max(previous[0], priority), max(previous[1], priority))
        last = ''
        for line in stream:
            fields = line.rstrip('\n').split('\t')
            assert len(fields) == 3, (mode, fields)
            phrase, traditional, simplified = fields
            assert phrase > last, (mode, phrase, last)
            last = phrase
            actual[phrase] = (int(traditional), int(simplified))
    assert actual == expected, (mode, len(actual), len(expected),
                                next(((key, value, actual.get(key)) for key, value in expected.items()
                                      if actual.get(key) != value), None))
    assert len(actual) > 1000
    assert any(traditional != simplified for traditional, simplified in actual.values())
    generated.append(actual)

assert generated[0] == generated[1], 'pinyin and zhuyin must use the same context evidence'
print(f'PASS: {len(generated[0]):,} generated context pairs use the correct script-specific priors')
