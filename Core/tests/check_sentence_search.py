#!/usr/bin/env python3
"""Diagnose word pruning before contextual scoring, without production word examples.

Default mode reports known search gaps and checks selection contracts. Use
--require-context-search to make those gaps fail (currently expected to fail).
This is a synthetic search invariant, not a language-quality benchmark.
"""
import argparse
import ctypes as C
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT / 'Core/MeowpuutCore.cpp')
    parser.add_argument('--require-context-search', action='store_true')
    parser.add_argument('--output', type=Path)
    options = parser.parse_args()
    results = []
    with tempfile.TemporaryDirectory(prefix='meowpuut-sentence-search-') as directory:
        library = Path(directory) / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
        subprocess.run(['c++', '-std=c++17', '-O2', '-shared', '-fPIC',
                        '-I' + str(ROOT / 'Core'), str(options.source), '-o', str(library)], check=True)
        api = C.CDLL(str(library))
        P, S, N, Z = C.c_void_p, C.c_char_p, C.c_int, C.c_size_t
        def bind(name, result, *args):
            function = getattr(api, 'mpn_' + name)
            function.restype, function.argtypes = result, args
            return function
        load = bind('lexicon_create', P, S, Z)
        drop = bind('lexicon_destroy', None, P)
        create = bind('session_create', P, P)
        destroy = bind('session_destroy', None, P)
        key = bind('key', N, P, N)
        hans = bind('set_simplified', None, P, N)
        count = bind('candidate_count', Z, P)
        candidate = bind('candidate', S, P, Z)
        consumed = bind('candidate_consumed', Z, P, Z)
        choose = bind('choose', N, P, Z)
        committed = bind('committed', S, P)
        raw_input = bind('input', S, P)
        for mode, head, head_reading, tail, tail_reading in [
            ('PINYIN', 'ma', 'ma3', 'de', 'de5'),
            ('ZHUYIN', 'su', 'su3', 'cl', 'cl3'),
        ]:
            for simplified in (0, 1):
                for offset in (0, 500, 1500):
                    # Six distinct homophones differ by only 50/700 points.
                    # The sixth has an existing 3-point contextual join, so
                    # exhaustive two-word scoring puts it first. No wider
                    # sentence beam is needed to retain this interpretation.
                    rows = [(head, head_reading, text, text, 8000 + offset - i * 10,
                             8000 + offset - i * 10) for i, text in enumerate('甲乙丙丁戊己')]
                    rows.append((tail, tail_reading, '庚', '庚', 8000, 8000))
                    rows.sort(key=lambda row: (row[0], -row[4]))
                    payload = (f'MEOWPUUT-{mode}-2\n' + ''.join('\t'.join(map(str, row)) + '\n' for row in rows)
                               + '# context\n己庚\t10000\t10000\n').encode()
                    lexicon = load(payload, len(payload))
                    assert lexicon
                    session = create(lexicon)
                    assert session
                    try:
                        hans(session, simplified)
                        raw = head + tail if mode == 'PINYIN' else head_reading + tail_reading
                        for byte in raw.encode():
                            assert key(session, byte)
                        choices = [(candidate(session, i).decode(), consumed(session, i))
                                   for i in range(count(session))]
                        assert choices and len(set(choices)) == len(choices), choices
                        assert all(0 < used <= len(raw) for _, used in choices), choices
                        expected = ('己庚', len(raw))
                        rank = choices.index(expected) + 1 if expected in choices else None
                        results.append({'mode': mode.lower(), 'simplified': bool(simplified),
                                        'prior_offset': offset, 'target_rank': rank,
                                        'full_candidates': [text for text, used in choices if used == len(raw)]})
                        # Also exercise a real returned selection: an absent
                        # preferred sentence must never corrupt its raw span.
                        index = next(i for i, (_, used) in enumerate(choices) if used == len(raw))
                        assert choose(session, index)
                        assert committed(session).decode() == choices[index][0]
                        assert raw_input(session) == b''
                    finally:
                        destroy(session)
                        drop(lexicon)
    gaps = sum(item['target_rank'] != 1 for item in results)
    report = {'scope': 'synthetic contextual-search diagnostic; not input accuracy',
              'cases': results, 'known_search_gaps': gaps, 'selection_contracts_passed': len(results)}
    if options.output:
        options.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(f'Selection contracts: {len(results)}/{len(results)}; contextual top-one gaps: {gaps}/{len(results)}')
    if gaps:
        print('KNOWN GAP: static homophone pruning happens before contextual join scoring.')
    if options.require_context_search and gaps:
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
