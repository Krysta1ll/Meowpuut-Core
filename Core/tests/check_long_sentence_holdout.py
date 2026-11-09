#!/usr/bin/env python3
"""Diagnostic sentence progression with real lexicons, not an accuracy benchmark.

These newly authored daily sentence combinations are a regression corpus, not
independent real-world language data or an accuracy benchmark. Quality gaps are reported,
not asserted away. Only input/selection contract violations make this command fail.
No dictionaries are generated or changed.
"""
import argparse
import ctypes
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
CASES = [
    ('keys_at_door', '我把钥匙放在门口了', 'wo3 ba3 yao4 shi5 fang4 zai4 men2 kou3 le5'),
    ('open_window', '请把窗户打开一点', 'qing3 ba3 chuang1 hu4 da3 kai1 yi4 dian3'),
    ('charge_phone', '下午记得给手机充电', 'xia4 wu3 ji4 de5 gei3 shou3 ji1 chong1 dian4'),
    ('borrow_book', '这本书可以借给我吗', 'zhe4 ben3 shu1 ke3 yi3 jie4 gei3 wo3 ma5'),
    ('weather_before_leaving', '明天出门之前看天气', 'ming2 tian1 chu1 men2 zhi1 qian2 kan4 tian1 qi4'),
    ('cat_sleeping', '猫在沙发下面睡觉', 'mao1 zai4 sha1 fa1 xia4 mian4 shui4 jiao4'),
    ('save_document', '把刚才的文件保存下来', 'ba3 gang1 cai2 de5 wen2 jian4 bao3 cun2 xia4 lai2'),
    ('walk_after_dinner', '我们吃完饭再去散步', 'wo3 men5 chi1 wan2 fan4 zai4 qu4 san4 bu4'),
    ('clear_photo', '这张照片拍得很清楚', 'zhe4 zhang1 zhao4 pian4 pai1 de5 hen3 qing1 chu3'),
    ('message_after_arrival', '到家以后给我发消息', 'dao4 jia1 yi3 hou4 gei3 wo3 fa1 xiao1 xi5'),
    ('slow_bus', '今天的公交车来得很慢', 'jin1 tian1 de5 gong1 jiao1 che1 lai2 de5 hen3 man4'),
    ('remember_address', '帮我把这个地址记下来', 'bang1 wo3 ba3 zhe4 ge5 di4 zhi3 ji4 xia4 lai2'),
]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def chinese(text):
    return bool(text) and all('\u3400' <= char <= '\u9fff' or 0x20000 <= ord(char) <= 0x323af for char in text)


def bind(core, name, args, result):
    fn = getattr(core, name)
    fn.argtypes, fn.restype = args, result
    return fn


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('resources', nargs='?', type=Path, default=ROOT / '.build/Meowpuut.app/Contents/Resources')
    parser.add_argument('--cases', type=Path, help='TSV: id, simplified target, toned syllables')
    parser.add_argument('--core-directory', type=Path, default=ROOT / 'Core', help='Optional frozen source directory')
    parser.add_argument('--output', type=Path, default=ROOT / '.build/long-sentence-progression.json')
    options = parser.parse_args()
    fixture_bytes = options.cases.read_bytes() if options.cases else None
    cases = [line.split('\t') for line in fixture_bytes.decode().splitlines()
             if line and not line.startswith('#')] if fixture_bytes is not None else CASES
    if not cases or any(len(case) != 3 or len(case[1]) < 4 for case in cases):
        parser.error('cases require three TSV columns and targets of at least four characters')
    if len({case[0] for case in cases}) != len(cases):
        parser.error('case identifiers must be unique')
    sys.path.insert(0, str(ROOT / 'scripts'))
    spec = importlib.util.spec_from_file_location('lexicon_encoding', ROOT / 'scripts/compile-native-lexicon.py')
    compiler = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(compiler)
    source = (options.core_directory / 'MeowpuutCore.cpp').read_bytes()
    header = (options.core_directory / 'MeowpuutCore.h').read_bytes()
    report = {'scope': 'core-only; curated diagnostic regression cases; ranks are not input accuracy',
              'exposure': 'Curated this round by the development agent; not human participant data, an independent usage sample or unseen validation.',
              'cases_sha256': sha(json.dumps(cases, ensure_ascii=False).encode()),
              'fixture_path': str(options.cases.resolve()) if options.cases else None,
              'fixture_sha256': sha(fixture_bytes) if fixture_bytes is not None else None,
              'core_sha256': sha(source), 'header_sha256': sha(header),
              'encoding_sha256': sha((ROOT / 'scripts/compile-native-lexicon.py').read_bytes()),
              'timing_scope': 'local per-key diagnostic timings; concurrent load uncontrolled, not a benchmark',
              'lexicons': {},
              'contract_failures': [], 'cases': []}
    with tempfile.TemporaryDirectory(prefix='meowpuut-long-holdout-') as directory:
        directory = Path(directory)
        (directory / 'MeowpuutCore.cpp').write_bytes(source)
        (directory / 'MeowpuutCore.h').write_bytes(header)
        library = directory / ('core.dylib' if sys.platform == 'darwin' else 'core.so')
        flags = ['-dynamiclib'] if sys.platform == 'darwin' else ['-shared', '-fPIC']
        subprocess.run(['c++', '-std=c++17', '-O2', *flags, str(directory / 'MeowpuutCore.cpp'), '-o', str(library)], check=True)
        core = ctypes.CDLL(str(library))
        P, S, Z, I = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int
        make = bind(core, 'mpn_lexicon_create', [S, Z], P)
        drop = bind(core, 'mpn_lexicon_destroy', [P], None)
        create = bind(core, 'mpn_session_create', [P], P)
        destroy = bind(core, 'mpn_session_destroy', [P], None)
        key = bind(core, 'mpn_key', [P, ctypes.c_int32], I)
        hans = bind(core, 'mpn_set_simplified', [P, I], None)
        count = bind(core, 'mpn_candidate_count', [P], Z)
        candidate = bind(core, 'mpn_candidate', [P, Z], S)
        consumed = bind(core, 'mpn_candidate_consumed', [P, Z], Z)
        choose = bind(core, 'mpn_choose', [P, Z], I)
        raw_input = bind(core, 'mpn_input', [P], S)
        committed = bind(core, 'mpn_committed', [P], S)
        clear_commit = bind(core, 'mpn_clear_commit', [P], None)
        clear = bind(core, 'mpn_clear', [P], None)

        def choices(session):
            return [{'text': candidate(session, index).decode(), 'consumed': consumed(session, index), 'index': index}
                    for index in range(count(session))]

        for mode in ('pinyin', 'zhuyin'):
            encoded = []
            wanted = set()
            for identity, target, reading in cases:
                syllables = reading.split()
                assert len(target) == len(syllables), identity
                tokens = [part[:-1] if mode == 'pinyin' else compiler.encode(part) for part in syllables]
                assert all(tokens), (identity, tokens)
                bounds = [0]
                for token in tokens:
                    bounds.append(bounds[-1] + len(token))
                raw = ''.join(tokens)
                for start in range(len(target)):
                    for end in range(start + 1, len(target) + 1):
                        wanted.add((raw[bounds[start]:bounds[end]], target[start:end]))
                encoded.append((identity, target, raw, bounds))
            data = (options.resources / f'native-{mode}.tsv').read_bytes()
            report['lexicons'][mode] = sha(data)
            found = set()
            seen_words = set()
            desired_words = {text for _, text in wanted}
            for line in data.decode().splitlines()[1:]:
                if line.startswith('# context'):
                    break
                fields = line.split('\t')
                if len(fields) < 5:
                    continue
                reading = fields[0 if mode == 'pinyin' else 1]
                text = fields[3]
                if text in desired_words:
                    seen_words.add(text)
                if (reading, text) in wanted:
                    found.add((reading, text))
            lexicon = make(data, len(data))
            assert lexicon, mode
            try:
                for identity, target, raw, bounds in encoded:
                    reachable = {0}
                    missing_characters = []
                    for start in range(len(target)):
                        pair = (raw[bounds[start]:bounds[start + 1]], target[start])
                        if pair not in found:
                            missing_characters.append({'text': target[start], 'reading': pair[0],
                                'word_exists_other_reading': target[start] in seen_words})
                        if start in reachable:
                            for end in range(start + 1, len(target) + 1):
                                if (raw[bounds[start]:bounds[end]], target[start:end]) in found:
                                    reachable.add(end)
                    case = {'id': identity, 'mode': mode, 'target': target, 'keys': raw,
                            'dictionary_segmentable': len(target) in reachable,
                            'missing_character_readings': missing_characters, 'steps': [], 'selection_checks': []}
                    report['cases'].append(case)
                    session = create(lexicon)
                    assert session
                    hans(session, 1)
                    best_seen = 0
                    longest_seen = 0
                    try:
                        for at, char in enumerate(raw, 1):
                            before = time.perf_counter_ns()
                            accepted = key(session, ord(char))
                            elapsed = (time.perf_counter_ns() - before) / 1e6
                            visible = choices(session)
                            if not accepted or raw_input(session).decode() != raw[:at]:
                                report['contract_failures'].append({'id': identity, 'mode': mode,
                                    'step': at, 'reason': 'input_not_preserved', 'input': raw_input(session).decode()})
                            correct = [entry for entry in visible if target.startswith(entry['text'])
                                       and entry['consumed'] <= at]
                            correct_chars = max((len(entry['text']) for entry in correct), default=0)
                            confirmed_chars = max((len(entry['text']) for entry in correct
                                if len(entry['text']) < len(bounds)
                                and entry['consumed'] == bounds[len(entry['text'])]), default=0)
                            clean = [entry for entry in visible if chinese(entry['text'])]
                            longest = max((len(entry['text']) for entry in clean), default=0)
                            step = {'key_count': at, 'input': raw[:at], 'latency_ms': round(elapsed, 3),
                                    'top5': visible[:5], 'candidate_count': len(visible),
                                    'max_chinese_consumed': max((entry['consumed'] for entry in clean), default=0),
                                    'max_chinese_characters': longest,
                                    'max_target_prefix_characters': correct_chars,
                                    'max_confirmed_target_prefix_characters': confirmed_chars,
                                    'target_prefix_regressed': best_seen >= 2 and correct_chars < best_seen,
                                    'lost_selectable_target_prefix': target[:best_seen] if best_seen >= 2 and correct_chars < best_seen else None,
                                    'all_candidates_fell_to_single_character': longest_seen >= 3 and longest <= 1}
                            case['steps'].append(step)
                            best_seen = max(best_seen, confirmed_chars)
                            longest_seen = max(longest_seen, longest)
                        full = [entry for entry in choices(session) if entry['text'] == target and entry['consumed'] == len(raw)]
                        case['target_rank'] = full[0]['index'] + 1 if full else None
                        case['quality_classification'] = 'target_available' if full else (
                            'ranking_or_search_gap' if case['dictionary_segmentable'] else 'reading_or_lexicon_gap')
                        # A target-aware greedy path estimates selection effort; it is
                        # neither the optimal path nor measured human interaction cost.
                        cost = {'strategy': 'longest correct prefix after full typing; target-aware greedy',
                                'status': 'incomplete', 'selections': [], 'seven_slot_page_advances': 0}
                        case['segmentation'] = cost
                        target_offset = 0
                        for _ in target:
                            remaining_target = target[target_offset:]
                            prefixes = [entry for entry in choices(session)
                                if entry['text'] and remaining_target.startswith(entry['text'])
                                and entry['consumed'] == bounds[target_offset + len(entry['text'])] - bounds[target_offset]]
                            if not prefixes:
                                cost['status'] = 'no_correct_prefix'
                                cost['blocked_input'] = raw_input(session).decode()
                                cost['blocked_top5'] = choices(session)[:5]
                                break
                            selected = max(prefixes, key=lambda entry: (len(entry['text']), -entry['index']))
                            next_offset = target_offset + len(selected['text'])
                            accepted = choose(session, selected['index'])
                            delivery = committed(session).decode()
                            suffix = raw_input(session).decode()
                            cost['selections'].append({'text': selected['text'], 'rank': selected['index'] + 1,
                                                       'consumed': selected['consumed'], 'remaining': suffix})
                            cost['seven_slot_page_advances'] += selected['index'] // 7
                            if not accepted or delivery != selected['text'] or suffix != raw[bounds[next_offset]:]:
                                cost['status'] = 'contract_failure'
                                report['contract_failures'].append({'id': identity, 'mode': mode,
                                    'reason': 'target_segmentation_contract', 'selection': selected,
                                    'actual_commit': delivery, 'actual_suffix': suffix})
                                break
                            clear_commit(session)
                            target_offset = next_offset
                            if target_offset == len(target):
                                cost['status'] = 'completed'
                                break
                        cost['selected_characters'] = target_offset
                        # Independently replay three non-final checkpoints and select the
                        # longest clean Chinese prefix available, regardless of its meaning.
                        # This tests the consumer contract without requiring a chosen sentence.
                        checkpoints = sorted({bounds[3] + 1, bounds[len(target) // 2] + 1, len(raw) - 1})
                        for checkpoint in checkpoints:
                            clear(session)
                            for char in raw[:checkpoint]:
                                assert key(session, ord(char)), (identity, mode, checkpoint)
                            partial = [entry for entry in choices(session) if chinese(entry['text'])
                                       and 0 < entry['consumed'] < checkpoint]
                            if not partial:
                                case['selection_checks'].append({'checkpoint': checkpoint, 'status': 'no_partial_candidate'})
                                continue
                            selection = max(partial, key=lambda item: (item['consumed'], -item['index']))
                            expected_tail = raw[selection['consumed']:checkpoint]
                            accepted = choose(session, selection['index'])
                            actual_commit, actual_tail = committed(session).decode(), raw_input(session).decode()
                            okay = bool(accepted) and actual_commit == selection['text'] and actual_tail == expected_tail
                            check = {'checkpoint': checkpoint, 'selected': selection, 'expected_tail': expected_tail,
                                     'actual_tail': actual_tail, 'status': 'passed' if okay else 'contract_failure'}
                            case['selection_checks'].append(check)
                            if not okay:
                                report['contract_failures'].append({'id': identity, 'mode': mode, **check})
                                continue
                            clear_commit(session)
                            for char in raw[checkpoint:]:
                                assert key(session, ord(char)), (identity, mode, checkpoint)
                            remainder = raw[selection['consumed']:]
                            check['continued_input'] = raw_input(session).decode()
                            check['continued_top5'] = choices(session)[:5]
                            if check['continued_input'] != remainder or committed(session):
                                check['status'] = 'contract_failure'
                                report['contract_failures'].append({'id': identity, 'mode': mode, **check})
                    finally:
                        destroy(session)
            finally:
                drop(lexicon)
    for mode in ('pinyin', 'zhuyin'):
        cases = [case for case in report['cases'] if case['mode'] == mode]
        print(mode, json.dumps({'sentences': len(cases), 'target_in_top5': sum(case['target_rank'] is not None and case['target_rank'] <= 5 for case in cases),
            'target_available': sum(case['target_rank'] is not None for case in cases),
            'dictionary_segmentable': sum(case['dictionary_segmentable'] for case in cases),
            'greedy_segmentation_completed': sum(case['segmentation']['status'] == 'completed' for case in cases),
            'greedy_selections_for_completed': sum(len(case['segmentation']['selections']) for case in cases if case['segmentation']['status'] == 'completed'),
            'seven_slot_page_advances_for_completed': sum(case['segmentation']['seven_slot_page_advances'] for case in cases if case['segmentation']['status'] == 'completed'),
            'partial_selection_checks_passed': sum(check['status'] == 'passed' for case in cases for check in case['selection_checks']),
            'checkpoints_without_partial_candidate': sum(check['status'] == 'no_partial_candidate' for case in cases for check in case['selection_checks']),
            'prefix_regression_cases': sum(any(step['target_prefix_regressed'] for step in case['steps']) for case in cases),
            'single_character_regression_cases': sum(any(step['all_candidates_fell_to_single_character'] for step in case['steps']) for case in cases)}, ensure_ascii=False))
    options.output.parent.mkdir(parents=True, exist_ok=True)
    options.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(f'contract failures: {len(report["contract_failures"])}; report: {options.output}')
    return bool(report['contract_failures'])


if __name__ == '__main__':
    sys.exit(main())
