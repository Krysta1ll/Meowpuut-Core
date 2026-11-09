#!/usr/bin/env python3
"""Syllable evidence and visible prefix choices must not depend on row crowding."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='meowpuut-syllable-evidence-') as temporary:
    source = Path(temporary) / 'check.cpp'
    binary = Path(temporary) / 'check'
    source.write_text('#include "' + str(root / 'Core/MeowpuutCore.cpp') + '"\n' + r'''
#include <cassert>
#include <fstream>
#include <iostream>
#include <iterator>
#include <set>

int main(int argc, char **argv) {
    std::string data = "MEOWPUUT-PINYIN-2\n";
    // Many phrase/abbreviation rows cannot hide a complete single syllable.
    for (int i = 0; i < 70; ++i)
        data += "ba\tba1\t多字\t多字\t" + std::to_string(10000 - i) + "\t" + std::to_string(10000 - i) + "\n";
    data += "ba\tba1\t八\t八\t100\t100\n";
    data += "ban\tban1\t班\t班\t100\t100\n";
    // Eight traditional variants merge to the same simplified prefix.
    const std::string variants[] = {"甲甲", "乙乙", "丙丙", "丁丁", "戊戊", "己己", "庚庚", "辛辛"};
    for (int i = 0; i < 8; ++i)
        data += "bani\tba1ni1\t" + variants[i] + "\t同一\t" + std::to_string(9000 - i) + "\t" + std::to_string(9000 - i) + "\n";
    data += "bani\tba1ni1\t同二\t同二\t8000\t8000\n";
    data += "bani\tba1ni1\t同三\t同三\t7000\t7000\n";
    data += "qu\tqu1\t多字\t多字\t9000\t9000\n";
    Lexicon fixture;
    assert(fixture.load(data));
    assert(!incomplete_pinyin_syllable(fixture, "ba"));
    assert(incomplete_pinyin_syllable(fixture, "b"));
    assert(!incomplete_pinyin_syllable(fixture, "qu"));
    assert(!incomplete_pinyin_syllable(fixture, "q"));
    for (const auto raw : {"baniq", "ba1ni1q"}) {
        MPNSession session;
        auto lexicon = std::make_shared<Lexicon>();
        assert(lexicon->load(data));
        session.lexicon = lexicon;
        session.hans = true;
        for (const char key : std::string(raw)) assert(mpn_key(&session, key));
        assert(session.candidates.size() >= 3);
        for (size_t i = 0; i < 3; ++i) {
            const std::string expected[] = {"同一", "同二", "同三"};
            assert(session.candidates[i].text == expected[i]);
            assert(session.candidates[i].consumed == std::string(raw).size() - 1);
        }
        assert(mpn_choose(&session, 1));
        assert(session.committed == "同二" && session.input == "q");
    }
    std::ifstream file(argv[1]);
    assert(file.good());
    Lexicon production;
    assert(production.load(std::string(std::istreambuf_iterator<char>(file), {})));
    // Independent exhaustive definition: all matching single-character
    // readings, regardless of frequency, script, source-row count or bucket.
    std::set<std::string> complete, probes{"", "zzzz", "quuuuu"};
    for (const auto &bucket : production.buckets) {
        if (bucket.key.size() > 6) continue;
        const auto range = production.range(bucket.key);
        for (size_t i = range.first; i < range.second; ++i) {
            if (fully_spelled(production.words[i]) && han_characters(production.words[i].text(false)).size() == 1) {
                complete.insert(std::string(bucket.key));
                for (size_t n = 1; n <= bucket.key.size(); ++n) probes.insert(std::string(bucket.key.substr(0, n)));
                break;
            }
        }
    }
    std::set<std::string> spellings;
    for (const auto &word : production.words) {
        const auto raw = word.reading();
        size_t start = 0;
        for (size_t end = 0; end < raw.size(); ++end) {
            if (!is_tone(raw[end], true)) continue;
            spellings.insert(std::string(raw.substr(start, end - start + 1)));
            start = end + 1;
        }
    }
    for (const auto &spelling : spellings) {
        bool expected = false;
        const auto range = production.range(std::string_view(spelling).substr(0, spelling.size() - 1));
        for (size_t i = range.first; i < range.second; ++i)
            if (production.words[i].reading() == spelling && han_characters(production.words[i].text(false)).size() == 1) expected = true;
        assert(std::binary_search(production.singleSpellings.begin(), production.singleSpellings.end(), spelling) == expected);
    }
    std::cout << "INDEX: " << production.singleReadings.size() << " bare, " << production.singleSpellings.size() << " toned, " << (production.singleReadings.capacity() + production.singleSpellings.capacity()) * sizeof(std::string_view) << " retained bytes\n";
    for (const auto &probe : probes) {
        bool expected = false;
        if (!probe.empty() && probe.size() <= 5 && !complete.count(probe))
            for (const auto &word : complete)
                if (word.size() > probe.size() && word.compare(0, probe.size(), probe) == 0) expected = true;
        assert(incomplete_pinyin_syllable(production, probe) == expected);
    }
    std::cout << "PASS: crowded syllables, distinct partial selections and " << probes.size() << " production syllable/prefix checks, " << spellings.size() << " spelling evidence checks\n";
}
''', encoding='utf-8')
    subprocess.run(['c++', '-std=c++17', '-O2', str(source), '-o', str(binary)], check=True)
    subprocess.run([str(binary), str(root / '.build/Meowpuut.app/Contents/Resources/native-pinyin.tsv')], check=True)
