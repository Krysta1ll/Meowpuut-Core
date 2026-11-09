#include "MeowpuutCore.h"

#include <algorithm>
#include <array>
#include <charconv>
#include <cmath>
#include <cstring>
#include <memory>
#include <numeric>
#include <string>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace {
constexpr std::string_view tones = " 6347";
constexpr std::string_view keys = "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/- 6347'";
constexpr std::array<std::string_view, 43> symbols = {
    "ㄅ","ㄆ","ㄇ","ㄈ","ㄉ","ㄊ","ㄋ","ㄌ","ㄍ","ㄎ","ㄏ","ㄐ","ㄑ","ㄒ","ㄓ","ㄔ","ㄕ","ㄖ","ㄗ","ㄘ","ㄙ",
    "ㄧ","ㄨ","ㄩ","ㄚ","ㄛ","ㄜ","ㄝ","ㄞ","ㄟ","ㄠ","ㄡ","ㄢ","ㄣ","ㄤ","ㄥ","ㄦ","ˉ","ˊ","ˇ","ˋ","˙"," "
};
static_assert(keys.size() == symbols.size());
constexpr size_t page_size = 7;
constexpr size_t max_input = 256;
constexpr size_t candidate_limit = 256;
// A single syllable can have hundreds of homophones. The regular sentence
// limit remains small; only exact single-character lookup may use this bound.
constexpr size_t single_character_candidate_limit = 2048;
constexpr size_t beam = 5;

bool is_tone(char key, bool pinyin = false) { return pinyin ? key >= '1' && key <= '5' : tones.find(key) != std::string_view::npos; }

// Context must contain only Han text. Punctuation, emoji, ASCII and malformed
// UTF-8 end the local phrase rather than carrying it into another composition.
std::vector<std::string_view> han_characters(std::string_view text) {
    std::vector<std::string_view> result;
    for (size_t start = 0; start < text.size();) {
        const auto first = static_cast<unsigned char>(text[start]);
        const size_t length = first >= 0xe0 && first <= 0xef ? 3 : first >= 0xf0 && first <= 0xf4 ? 4 : 0;
        if (!length || start + length > text.size()) return {};
        uint32_t scalar = first & (length == 3 ? 0x0f : 0x07);
        for (size_t i = 1; i < length; ++i) {
            const auto next = static_cast<unsigned char>(text[start + i]);
            if ((next & 0xc0) != 0x80) return {};
            scalar = (scalar << 6) | (next & 0x3f);
        }
        if ((length == 4 && scalar < 0x10000) || !((scalar >= 0x3400 && scalar <= 0x9fff) || (scalar >= 0xf900 && scalar <= 0xfaff) ||
              (scalar >= 0x20000 && scalar <= 0x323af))) return {};
        result.push_back(text.substr(start, length));
        start += length;
    }
    return result;
}


// Check the single-character case without allocating a vector during lexicon loading.
bool single_han(std::string_view text) {
    if (text.size() != 3 && text.size() != 4) return false;
    const auto first = static_cast<unsigned char>(text[0]);
    const size_t length = first >= 0xe0 && first <= 0xef ? 3 : first >= 0xf0 && first <= 0xf4 ? 4 : 0;
    if (length != text.size()) return false;
    uint32_t scalar = first & (length == 3 ? 0x0f : 0x07);
    for (size_t i = 1; i < length; ++i) {
        const auto next = static_cast<unsigned char>(text[i]);
        if ((next & 0xc0) != 0x80) return false;
        scalar = (scalar << 6) | (next & 0x3f);
    }
    return !(length == 4 && scalar < 0x10000) &&
        ((scalar >= 0x3400 && scalar <= 0x9fff) || (scalar >= 0xf900 && scalar <= 0xfaff) ||
         (scalar >= 0x20000 && scalar <= 0x323af));
}

struct Word {
    std::string_view row;
    int weight;
    int hansWeight;

    int score(bool hans) const { return hans ? hansWeight : weight; }

    std::string_view reading() const { return row.substr(0, row.find('\t')); }
    std::string_view text(bool hans) const {
        const auto start = row.find('\t') + 1;
        const auto separator = row.find('\t', start);
        return hans ? row.substr(separator + 1) : row.substr(start, separator - start);
    }
};

bool fully_spelled(const Word &word) {
    bool vowel = false;
    for (char key : word.reading()) {
        if (is_tone(key, true)) {
            if (!vowel) return false;
            vowel = false;
        } else if (std::string_view("aeiouv").find(key) != std::string_view::npos) {
            vowel = true;
        }
    }
    return true;
}

struct Lexicon {
    // The table is immutable after loading. Row and index views point into it.
    std::string storage;
    std::vector<Word> words;
    // Bucket offsets remain shared, but simplified frequency can order the
    // same readings differently without disturbing traditional candidates.
    std::vector<uint32_t> hansOrder;
    struct Bucket { std::string_view key; size_t start; };
    std::vector<Bucket> buckets;
    // Sorted independent syllable evidence; frequency and abbreviation rows
    // must not decide whether an input is a complete Pinyin syllable.
    std::vector<std::string_view> singleReadings, singleSpellings;
    struct ContextScore { std::string_view first; int weight, hansWeight; };
    std::vector<ContextScore> context;
    std::unordered_map<std::string_view, std::pair<std::string_view, int>> traditionalCharacters;
    size_t longest = 0;
    bool pinyin = false;
    std::array<std::vector<size_t>, 128> initials;
    std::array<std::vector<size_t>, 128> hansInitials;

    const Word &word(size_t position, bool hans) const {
        return words[hans && !hansOrder.empty() ? hansOrder[position] : position];
    }

    bool load(std::string_view data) {
        storage.assign(data);
        data = storage;
        const bool version2 = data.substr(0, std::string_view("MEOWPUUT-PINYIN-2\n").size()) == "MEOWPUUT-PINYIN-2\n" ||
                              data.substr(0, std::string_view("MEOWPUUT-ZHUYIN-2\n").size()) == "MEOWPUUT-ZHUYIN-2\n";
        pinyin = data.substr(0, std::string_view("MEOWPUUT-PINYIN-1\n").size()) == "MEOWPUUT-PINYIN-1\n" ||
                 data.substr(0, std::string_view("MEOWPUUT-PINYIN-2\n").size()) == "MEOWPUUT-PINYIN-2\n";
        const std::string_view header = pinyin ? (version2 ? "MEOWPUUT-PINYIN-2\n" : "MEOWPUUT-PINYIN-1\n")
                                                : (version2 ? "MEOWPUUT-ZHUYIN-2\n" : "MEOWPUUT-ZHUYIN-1\n");
        if (data.substr(0, header.size()) != header) return false;
        data.remove_prefix(header.size());
        words.reserve(std::count(data.begin(), data.end(), '\n'));
        bool reading_context = false;
        while (!data.empty()) {
            const auto newline = data.find('\n');
            auto line = data.substr(0, newline);
            if (newline == data.npos) data = {};
            else data.remove_prefix(newline + 1);
            if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
            if (line.empty()) continue;
            if (line == "# context" && !reading_context) {
                reading_context = true;
                continue;
            }
            const auto first_tab = line.find('\t');
            if (first_tab == line.npos || first_tab == 0) return false;
            const auto bare = line.substr(0, first_tab);
            if (reading_context) {
                const auto second_tab = line.find('\t', first_tab + 1);
                if ((!version2 && second_tab != line.npos) ||
                    (second_tab != line.npos && line.find('\t', second_tab + 1) != line.npos) ||
                    (!context.empty() && context.back().first >= bare)) return false;
                const auto parseScore = [](std::string_view value, int &score) {
                    const auto parsed = std::from_chars(value.data(), value.data() + value.size(), score);
                    return parsed.ec == std::errc{} && parsed.ptr == value.data() + value.size() && score >= 0;
                };
                const auto scoreEnd = second_tab == line.npos ? line.size() : second_tab;
                int weight = 0, hansWeight = 0;
                if (!parseScore(line.substr(first_tab + 1, scoreEnd - first_tab - 1), weight)) return false;
                hansWeight = weight;
                if (second_tab != line.npos && !parseScore(line.substr(second_tab + 1), hansWeight)) return false;
                context.push_back({bare, weight, hansWeight});
                continue;
            }
            const auto last_tab = line.rfind('\t');
            const auto weight_tab = version2 ? line.rfind('\t', last_tab - 1) : last_tab;
            if (weight_tab == line.npos || (version2 && weight_tab <= first_tab)) return false;
            const auto number = line.substr(weight_tab + 1, (version2 ? last_tab : line.size()) - weight_tab - 1);
            int weight = 0;
            const auto end = number.data() + number.size();
            const auto parsed = std::from_chars(number.data(), end, weight);
            if (parsed.ec != std::errc{} || parsed.ptr != end || weight < 0) return false;

            if (weight_tab == first_tab) return false;
            int hansWeight = weight;
            if (version2) {
                const auto number = line.substr(last_tab + 1);
                const auto parsed = std::from_chars(number.data(), number.data() + number.size(), hansWeight);
                if (parsed.ec != std::errc{} || parsed.ptr != number.data() + number.size() || hansWeight < 0) return false;
            }
            const auto row = line.substr(first_tab + 1, weight_tab - first_tab - 1);
            const Word word{row, weight, hansWeight};
            const auto reading = word.reading();
            const auto first_separator = row.find('\t');
            const auto second_separator = row.find('\t', first_separator + 1);
            if (reading.empty() || first_separator == row.npos || second_separator == row.npos ||
                first_separator + 1 == second_separator || second_separator + 1 == row.size() ||
                row.find('\t', second_separator + 1) != row.npos ||
                !is_tone(reading.back(), pinyin)) return false;
            size_t position = 0;
            bool has_sound = false;
            for (char key : reading) {
                if (is_tone(key, pinyin)) {
                    if (!has_sound) return false;
                    has_sound = false;
                } else {
                    if (position == bare.size() || key != bare[position++] ||
                        (pinyin ? key < 'a' || key > 'z' : keys.find(key) == keys.npos) || key == '\'') return false;
                    has_sound = true;
                }
            }
            if (position != bare.size() || bare.size() > max_input) return false;
            if (buckets.empty() || buckets.back().key != bare) {
                if (!buckets.empty() && buckets.back().key > bare) return false;
                buckets.push_back({bare, words.size()});
                longest = std::max(longest, bare.size());
            } else if (words.back().weight < weight) return false;
            words.push_back(word);
        }
        {
            if (version2) {
                if (words.size() > UINT32_MAX) return false;
                hansOrder.resize(words.size());
                std::iota(hansOrder.begin(), hansOrder.end(), uint32_t{0});
                for (size_t i = 0; i < buckets.size(); ++i) {
                    const size_t end = i + 1 == buckets.size() ? words.size() : buckets[i + 1].start;
                    std::stable_sort(hansOrder.begin() + buckets[i].start, hansOrder.begin() + end,
                        [&](uint32_t a, uint32_t b) { return words[a].hansWeight > words[b].hansWeight; });
                }
            }
            if (pinyin) {
                for (size_t i = 0; i < buckets.size(); ++i) {
                    if (buckets[i].key.size() > 6) continue;
                    const size_t end = i + 1 == buckets.size() ? words.size() : buckets[i + 1].start;
                    for (size_t j = buckets[i].start; j < end; ++j) {
                        if (fully_spelled(words[j]) && single_han(words[j].text(false))) {
                            singleReadings.push_back(buckets[i].key);
                            break;
                        }
                    }
                }
            }
            for (size_t i = 0; i < words.size(); ++i) {
                const auto reading = words[i].reading();
                const auto traditional = words[i].text(false), simplified = words[i].text(true);
                if (pinyin && traditional.size() <= 4 && single_han(traditional))
                    singleSpellings.push_back(reading);
                if (traditional.size() <= 4 && simplified.size() <= 4 &&
                    single_han(traditional) && single_han(simplified)) {
                    auto found = traditionalCharacters.find(simplified);
                    if (found == traditionalCharacters.end() || found->second.second < words[i].weight)
                        traditionalCharacters[simplified] = {traditional, words[i].weight};
                }
                if (words[i].text(false).size() <= 4 && static_cast<unsigned char>(reading.front()) < initials.size()) {
                    const auto initial = static_cast<unsigned char>(reading.front());
                    initials[initial].push_back(i);
                    hansInitials[initial].push_back(i);
                }
            }
            std::sort(singleSpellings.begin(), singleSpellings.end());
            singleSpellings.erase(std::unique(singleSpellings.begin(), singleSpellings.end()), singleSpellings.end());
            singleSpellings.shrink_to_fit();
            singleReadings.shrink_to_fit();
            auto prepareInitials = [&](auto &groups, bool hans) {
                for (auto &items : groups) {
                    std::stable_sort(items.begin(), items.end(), [&](size_t a, size_t b) {
                        return words[a].score(hans) > words[b].score(hans);
                    });
                    std::unordered_set<std::string_view> visible;
                    size_t retained = 0;
                    for (const auto index : items) {
                        if (visible.insert(words[index].text(hans)).second) items[retained++] = index;
                        if (retained == 64) break;
                    }
                    items.resize(retained);
                }
            };
            prepareInitials(initials, false);
            prepareInitials(hansInitials, true);
        }
        return !words.empty();
    }

    double join_bonus(std::string_view left, std::string_view right, bool hans, bool distinguishStrong = false) const {
        if (context.empty() || right.empty()) return 0;
        // UTF-8 character boundaries for up to four characters on each side.
        std::array<size_t, 4> starts{}, ends{};
        size_t left_count = 0, right_count = 0;
        const auto is_start = [](char byte) { return (static_cast<unsigned char>(byte) & 0xC0) != 0x80; };
        for (size_t i = left.size(); i > 0 && left_count < starts.size();) {
            if (is_start(left[--i])) starts[left_count++] = i;
        }
        for (size_t i = 1; i <= right.size() && right_count < ends.size(); ++i) {
            if (i == right.size() || is_start(right[i])) ends[right_count++] = i;
        }
        if (left_count == 0 || right_count == 0) return 0;
        const size_t offset = starts[left_count - 1];
        std::string joined(left.substr(offset));
        joined.append(right.substr(0, ends[right_count - 1]));
        int frequency = 0;
        for (size_t i = 0; i < left_count; ++i) {
            const size_t begin = starts[i] - offset;
            for (size_t j = 0; j < right_count && i + j < 4; ++j) {
                const auto phrase = std::string_view(joined).substr(begin, left.size() - starts[i] + ends[j]);
                const auto entry = std::lower_bound(context.begin(), context.end(), phrase,
                    [](const auto &item, std::string_view value) { return item.first < value; });
                if (entry != context.end() && entry->first == phrase)
                    frequency = std::max(frequency, hans ? entry->hansWeight : entry->weight);
            }
        }
        // Multi-character continuations provide more evidence than a single
        // potentially polyphonic character. For segment re-ranking, retain
        // differences between strong frequencies instead of clipping them all
        // to the same score. Decoder joins and single-character order retain
        // their existing scale.
        if (distinguishStrong && right_count > 1)
            return std::min(3.0, 3.0 * std::log1p(frequency) / std::log1p(10000.0));
        return std::min(3.0, 0.4 * std::log1p(frequency));
    }

    std::string traditionalize(std::string_view value, bool tail) const {
        const auto characters = han_characters(value);
        if (characters.empty()) return {};
        std::string result;
        const size_t start = tail && characters.size() > 4 ? characters.size() - 4 : 0;
        const size_t end = std::min(characters.size(), start + 4);
        for (size_t i = start; i < end; ++i) {
            const auto entry = traditionalCharacters.find(characters[i]);
            result.append(entry == traditionalCharacters.end() ? characters[i] : entry->second.first);
        }
        return result;
    }

    std::pair<size_t, size_t> range(std::string_view key) const {
        const auto bucket = std::lower_bound(buckets.begin(), buckets.end(), key,
                                             [](const Bucket &item, std::string_view reading) {
                                                 return item.key < reading;
                                             });
        if (bucket == buckets.end() || bucket->key != key) return {0, 0};
        const auto next = bucket + 1;
        return {bucket->start, next == buckets.end() ? words.size() : next->start};
    }
};

struct Reading {
    std::string sounds;
    std::array<char, max_input + 1> boundaries{};
    std::array<size_t, max_input + 1> boundary_count{};

    bool pinyin;
    bool unambiguous = true;
    explicit Reading(std::string_view input, bool latin) : pinyin(latin) {
        for (char key : input) {
            if (is_tone(key, pinyin) || key == '\'') {
                if (sounds.empty() || boundaries[sounds.size()]) unambiguous = false;
                boundaries[sounds.size()] = key;
            }
            else sounds += key;
        }
        for (size_t i = 1; i <= sounds.size(); ++i) {
            boundary_count[i] = boundary_count[i - 1] + (boundaries[i] != 0);
        }
    }

    bool matches(const Word &word, size_t start, size_t end) const {
        const size_t required = boundary_count[end] - boundary_count[start];
        if (required == 0) return true;
        size_t matched = 0;
        for (char key : word.reading()) {
            if (!is_tone(key, pinyin)) { ++start; continue; }
            const char tone = boundaries[start];
            if (tone && tone != key && tone != '\'') return false;
            matched += tone != 0;
        }
        return matched == required;
    }
};

struct Fallback { std::string text; size_t consumed; double score; };

bool has_exact_match(const Lexicon &lexicon, std::string_view input) {
    const Reading reading(input, lexicon.pinyin);
    const auto range = lexicon.range(reading.sounds);
    for (size_t i = range.first; i < range.second; ++i)
        if (reading.matches(lexicon.words[i], 0, reading.sounds.size())) return true;
    return false;
}

// One bounded n/ng correction at a pinyin syllable ending. Dictionary
// validation keeps arbitrary edits out of the candidate list.
std::vector<std::string> nasal_variants(std::string_view sounds) {
    std::vector<std::string> result;
    if (sounds.size() < 4 || sounds.size() > 32) return result;
    for (size_t i = 1; i < sounds.size(); ++i) {
        if (sounds[i] != 'n' || std::string_view("aeiouv").find(sounds[i - 1]) == std::string_view::npos) continue;
        auto variant = std::string(sounds);
        if (i + 1 < sounds.size() && sounds[i + 1] == 'g') variant.erase(i + 1, 1);
        else if (i + 1 == sounds.size() || std::string_view("aeiouv").find(sounds[i + 1]) == std::string_view::npos)
            variant.insert(i + 1, 1, 'g');
        else continue;
        result.push_back(std::move(variant));
    }
    return result;
}

// When the last syllable is still being typed, keep a completed phrase
// selectable without consuming the unfinished suffix. Whole-word matches
// continue to take precedence over this partial conversion.
std::vector<Fallback> unfinished_phrase(const Lexicon &lexicon, std::string_view input, bool hans) {
    if (!lexicon.pinyin || input.size() < 3 || input.size() > 64) return {};
    if (has_exact_match(lexicon, input)) return {};

    for (size_t tail = 1; tail <= std::min(size_t(3), input.size() - 1); ++tail) {
        const auto suffix = input.substr(input.size() - tail);
        if (!std::all_of(suffix.begin(), suffix.end(), [](char c) { return c >= 'a' && c <= 'z'; })) continue;
        // A complete syllable or word can already participate in sentence decoding.
        const auto completeSuffix = lexicon.range(suffix);
        if (completeSuffix.first != completeSuffix.second) continue;
        const size_t count = input.size() - tail;
        const Reading prefix(input.substr(0, count), true);
        const auto range = lexicon.range(prefix.sounds);
        std::vector<Fallback> matches;
        std::unordered_set<std::string_view> visible;
        for (size_t i = range.first; i < range.second && matches.size() < 3; ++i) {
            const auto &word = lexicon.word(i, hans);
            const auto text = word.text(hans);
            const auto characters = std::count_if(text.begin(), text.end(), [](unsigned char c) { return (c & 0xc0) != 0x80; });
            // The table also contains consonant-only abbreviations (e.g. wo
            // for 玩偶). They are predictions, not a completed pinyin phrase.
            bool hasVowel = false, complete = true;
            for (char key : word.reading()) {
                if (is_tone(key, true)) { complete = complete && hasVowel; hasVowel = false; }
                else if (std::string_view("aeiouv").find(key) != std::string_view::npos) hasVowel = true;
            }
            if (complete && characters > 1 && prefix.matches(word, 0, prefix.sounds.size()) && visible.insert(text).second)
                matches.push_back({std::string(text), count, static_cast<double>(word.score(hans))});
        }
        if (!matches.empty()) return matches;
    }
    return {};
}

std::vector<Fallback> fallback(const Lexicon &lexicon, std::string_view input, bool hans, bool allowSingle = true) {
    std::vector<Fallback> matches;
    const Reading reading(input, lexicon.pinyin);
    const auto &sounds = reading.sounds;
    auto collect = [&](std::string_view key, double penalty) {
        const auto range = lexicon.range(key);
        std::unordered_set<std::string_view> visible;
        // Distinct traditional entries can share one simplified spelling.
        // Bound work but count visible choices, not duplicate source rows.
        for (size_t i = range.first; i < range.second && i - range.first < 256 && visible.size() < 7; ++i) {
            const auto &word = lexicon.word(i, hans);
            const auto text = word.text(hans);
            if (visible.insert(text).second)
                matches.push_back({std::string(text), input.size(), word.score(hans) - penalty});
        }
    };
    if (sounds.size() == 1 && static_cast<unsigned char>(sounds[0]) < lexicon.initials.size()) {
        const auto initial = static_cast<unsigned char>(sounds[0]);
        const auto &items = hans ? lexicon.hansInitials[initial] : lexicon.initials[initial];
        for (size_t i : items) {
            const auto &word = lexicon.words[i];
            matches.push_back({std::string(word.text(hans)), input.size(), double(word.score(hans))});
        }
    }
    if (lexicon.pinyin && sounds.size() >= 2 && sounds.size() <= 32 && input == sounds) {
        // Bounded prefix completion: binary search, then at most 256 dictionary buckets.
        auto first = std::lower_bound(lexicon.buckets.begin(), lexicon.buckets.end(), sounds,
            [](const Lexicon::Bucket &bucket, std::string_view key) { return bucket.key < key; });
        size_t scanned = 0;
        for (auto it = first; it != lexicon.buckets.end() && scanned++ < 256; ++it) {
            if (it->key.substr(0, sounds.size()) != sounds) break;
            if (it->key.size() - sounds.size() <= 6) collect(it->key, 1200.0 * (it->key.size() - sounds.size()));
        }
        // One local correction only; no full dictionary scan or arbitrary multi-edit guesses.
        if (sounds.size() >= 4) {
            for (size_t i = 0; i < sounds.size(); ++i) {
                if (i + 1 < sounds.size()) { auto key = sounds; std::swap(key[i], key[i + 1]); collect(key, 800); }
                if (i && sounds[i] == sounds[i - 1]) { auto key = sounds; key.erase(i, 1); collect(key, 800); }
                std::string_view neighbors;
                switch (sounds[i]) {
                    case 'q': neighbors = "wa"; break; case 'w': neighbors = "qeas"; break;
                    case 'e': neighbors = "wrsd"; break; case 'r': neighbors = "etdf"; break;
                    case 't': neighbors = "ryfg"; break; case 'y': neighbors = "tugh"; break;
                    case 'u': neighbors = "yihj"; break; case 'i': neighbors = "uojk"; break;
                    case 'o': neighbors = "ipkl"; break; case 'p': neighbors = "ol"; break;
                    case 'a': neighbors = "qwsz"; break; case 's': neighbors = "awedzx"; break;
                    case 'd': neighbors = "serfxc"; break; case 'f': neighbors = "drtgcv"; break;
                    case 'g': neighbors = "ftyhbv"; break; case 'h': neighbors = "gyujbn"; break;
                    case 'j': neighbors = "huiknm"; break; case 'k': neighbors = "jiolm"; break;
                    case 'l': neighbors = "kop"; break; case 'z': neighbors = "asx"; break;
                    case 'x': neighbors = "zsdc"; break; case 'c': neighbors = "xdfv"; break;
                    case 'v': neighbors = "cfgb"; break; case 'b': neighbors = "vghn"; break;
                    case 'n': neighbors = "bhjm"; break; case 'm': neighbors = "njk"; break;
                    default: break;
                }
                for (char ch : neighbors) { auto key = sounds; key[i] = ch; collect(key, 1200); }
            }
            for (const auto &variant : nasal_variants(sounds)) collect(variant, 900);
        }
    }
    if (matches.empty() && allowSingle) {
        // Offer the first character only, retaining every unconverted key after selection.
        for (size_t count = std::min(input.size(), size_t(7)); count > 0; --count) {
            // A tone or explicit separator completes the preceding syllable.
            // Never select its untoned prefix and leave an orphaned boundary.
            if (count < input.size() && (is_tone(input[count], lexicon.pinyin) || input[count] == '\'')) continue;
            const Reading prefix(input.substr(0, count), lexicon.pinyin);
            if (prefix.sounds.empty()) continue;
            const auto range = lexicon.range(prefix.sounds);
            std::unordered_set<std::string_view> visible;
            for (size_t i = range.first; i < range.second && i - range.first < 512 && visible.size() < 28; ++i) {
                const auto &word = lexicon.word(i, hans);
                const auto text = word.text(hans);
                const auto characters = std::count_if(text.begin(), text.end(), [](unsigned char ch) { return (ch & 0xc0) != 0x80; });
                if (characters == 1 && prefix.matches(word, 0, prefix.sounds.size()) && visible.insert(text).second)
                    matches.push_back({std::string(text), count, static_cast<double>(word.score(hans))});
            }
            if (!matches.empty()) break;
        }
    }
    std::stable_sort(matches.begin(), matches.end(), [](const Fallback &a, const Fallback &b) { return a.score > b.score; });
    std::unordered_set<std::string> seen;
    std::vector<Fallback> result;
    for (auto &match : matches) {
        if (seen.insert(match.text).second) result.push_back(std::move(match));
        if (result.size() == 28) break;
    }
    return result;
}

struct CandidateMetadata {
    uint32_t kind = MPN_CANDIDATE_UNKNOWN;
    uint32_t scoreKind = MPN_SCORE_NONE;
    double score = 0;
};
struct Candidate {
    std::string text;
    size_t consumed;
    CandidateMetadata metadata;
    // Only Android's complete-reading presentation appends a raw suffix.
    // Selection commits the converted prefix and retains this suffix.
    size_t literalTail = 0;
};

std::vector<std::string> decode(const Lexicon &lexicon, std::string_view input, bool hans, bool completeOnly = false,
                                std::vector<CandidateMetadata> *metadata = nullptr, bool *singleCharacterReading = nullptr) {
    const Reading reading(input, lexicon.pinyin);
    const size_t length = reading.sounds.size();
    if (length == 0) return {};

    std::vector<std::string> results;
    std::unordered_set<std::string> seen;
    // Vowels alone do not prove that an abbreviation is a pinyin syllable.
    // Require independent single-character dictionary evidence for each
    // syllable. Missing evidence is UNKNOWN, not a guessed correction.
    auto verifiedSpelling = [&](const Word &word) {
        if (!metadata || !lexicon.pinyin) return true;
        const auto spelling = word.reading();
        size_t start = 0;
        for (size_t end = 0; end < spelling.size(); ++end) {
            if (!is_tone(spelling[end], true)) continue;
            const auto syllable = spelling.substr(start, end - start + 1);
            if (!std::binary_search(lexicon.singleSpellings.begin(), lexicon.singleSpellings.end(), syllable)) return false;
            start = end + 1;
        }
        return start == spelling.size();
    };
    auto append = [&](std::string_view text, CandidateMetadata item) {
        if (seen.emplace(text).second) {
            results.emplace_back(text);
            if (metadata) metadata->push_back(item);
        }
    };
    const auto whole = lexicon.range(reading.sounds);
    const bool singleReading = !completeOnly && metadata && std::any_of(
        lexicon.words.begin() + whole.first, lexicon.words.begin() + whole.second,
        [&](const Word &word) {
            const auto spelling = word.reading();
            return std::count_if(spelling.begin(), spelling.end(),
                       [&](char key) { return is_tone(key, lexicon.pinyin); }) == 1 &&
                   han_characters(word.text(false)).size() == 1 &&
                   reading.matches(word, 0, length);
        });
    if (singleCharacterReading) *singleCharacterReading = singleReading;
    const auto result_limit = singleReading ? single_character_candidate_limit : candidate_limit;
    for (size_t index = whole.first; index < whole.second; ++index) {
        const auto &word = lexicon.word(index, hans);
        if ((!completeOnly || fully_spelled(word)) && reading.matches(word, 0, length)) {
            const auto kind = !reading.unambiguous ? MPN_CANDIDATE_UNKNOWN :
                (lexicon.pinyin && !fully_spelled(word) ? MPN_CANDIDATE_CORRECTION :
                 (verifiedSpelling(word) ? MPN_CANDIDATE_EXACT : MPN_CANDIDATE_UNKNOWN));
            append(word.text(hans), {kind, MPN_SCORE_LEXICON, double(word.score(hans))});
        }
        if (results.size() == result_limit) return results;
    }

    // Keep the first page of exact single-syllable characters intact: a
    // nasal correction like ban should not displace bang's common characters.
    // For longer readings, surface likely corrections after three exact words.
    if (!completeOnly && lexicon.pinyin && !results.empty() && input == reading.sounds) {
        size_t insertion = std::min(singleReading ? page_size : size_t(3), results.size());
        size_t added = 0;
        for (const auto &variant : nasal_variants(reading.sounds)) {
            const auto range = lexicon.range(variant);
            for (size_t index = range.first; index < range.second && added < 2; ++index) {
                const auto &word = lexicon.word(index, hans);
                if (word.score(hans) < 4000) break;
                const auto text = word.text(hans);
                if (seen.emplace(text).second) {
                    results.insert(results.begin() + insertion, std::string(text));
                    if (metadata) metadata->insert(metadata->begin() + insertion, {MPN_CANDIDATE_CORRECTION, MPN_SCORE_LEXICON, double(word.score(hans))});
                    ++insertion; ++added;
                }
            }
            if (added == 2) break;
        }
        if (results.size() > result_limit) { results.resize(result_limit); if (metadata) metadata->resize(result_limit); }
    }

    // Whole dictionary words stay first; known completions/corrections precede
    // speculative chains such as interpreting a mistyped ending as an abbreviation.
    if (results.empty() && lexicon.pinyin && !completeOnly) {
        for (const auto &item : fallback(lexicon, input, hans, false))
            append(item.text, {MPN_CANDIDATE_CORRECTION, MPN_SCORE_FALLBACK, item.score});
    }

    struct Path { std::string text, traditional; double score; bool spelled, verified; };
    std::vector<std::vector<Path>> suffixes(length + 1);
    suffixes[length].push_back({"", "", 0, true, true});
    for (size_t start = length; start-- > 0;) {
        std::vector<Path> paths;
        for (size_t end = start + 1; end <= std::min(length, start + lexicon.longest); ++end) {
            if (suffixes[end].empty() || (start == 0 && end == length)) continue;
            const auto options = lexicon.range(std::string_view(reading.sounds).substr(start, end - start));
            std::vector<std::string_view> used_spellings;
            used_spellings.reserve(beam);
            for (size_t index = options.first; index < options.second; ++index) {
                const auto &word = lexicon.word(index, hans);
                if (completeOnly && !fully_spelled(word)) continue;
                if (!reading.matches(word, start, end)) continue;
                // Tone and traditional variants remain available as exact
                // candidates. In a bounded sentence beam, variants with the
                // same simplified form share one slot so they do not crowd
                // out different words and sentence interpretations.
                const auto spelling = word.text(true);
                if (std::find(used_spellings.begin(), used_spellings.end(), spelling) != used_spellings.end()) continue;
                used_spellings.push_back(spelling);
                // Compiler weights are already log-frequency priors. Logging them again
                // erases the distinction between common phrases and rare fragments.
                const double score = static_cast<double>(word.score(hans)) / 700.0 - 19.0;
                const bool verified = verifiedSpelling(word);
                for (const auto &suffix : suffixes[end]) {
                    const auto traditional = word.text(false);
                    const auto &right = hans ? suffix.traditional : suffix.text;
                    const double bonus = lexicon.join_bonus(traditional, right, hans);
                    paths.push_back({
                        std::string(word.text(hans)) + suffix.text,
                        hans ? std::string(traditional) + right : std::string(),
                        score + suffix.score + bonus,
                        suffix.spelled && (!lexicon.pinyin || fully_spelled(word)),
                        suffix.verified && verified,
                    });
                }
                if (used_spellings.size() == beam) break;
            }
        }
        std::sort(paths.begin(), paths.end(), [](const Path &a, const Path &b) {
            return a.score == b.score ? a.text < b.text : a.score > b.score;
        });
        std::unordered_set<std::string> unique;
        for (auto &path : paths) {
            if (unique.insert(path.text).second) suffixes[start].push_back(std::move(path));
            if (suffixes[start].size() == beam) break;
        }
    }
    for (const auto &path : suffixes[0]) {
        const auto kind = !reading.unambiguous ? MPN_CANDIDATE_UNKNOWN :
            (!path.spelled ? MPN_CANDIDATE_CORRECTION :
             (path.verified ? MPN_CANDIDATE_PATH : MPN_CANDIDATE_UNKNOWN));
        append(path.text, {kind, MPN_SCORE_PATH, path.score});
        if (results.size() == result_limit) break;
    }
    return results;
}

// An unfinished initial can disambiguate the previous character: ㄒㄧㄏ
// can continue as 喜歡, for instance. Consult bounded dictionary completions
// rather than promoting a single context-free homophone.
std::vector<Fallback> predictive_segments_at(const Lexicon &lexicon, std::string_view input, bool hans, size_t tail) {
    if (input.size() < 4 || input.size() > 64 || has_exact_match(lexicon, input)) return {};
    const size_t count = input.size() - tail;
    const auto suffix = input.substr(count);
    if (lexicon.pinyin) {
        if (std::string_view("bpmdtlgkhjqxzcrsfyw").find(suffix.front()) == std::string_view::npos ||
            !std::all_of(suffix.begin(), suffix.end(), [](char c) { return c >= 'a' && c <= 'z'; })) return {};
    } else if (keys.substr(0, 21).find(suffix.front()) == std::string_view::npos ||
               std::any_of(suffix.begin(), suffix.end(), [](char c) { return is_tone(c) || c == '\''; })) return {};
    if (input[count - 1] == '\'') return {};
    auto phrases = decode(lexicon, input.substr(0, count), hans, lexicon.pinyin);
    if (phrases.empty()) return {};
    struct Scored { Fallback item; int evidence; size_t order; };
    std::vector<Scored> scored;
    for (size_t i = 0; i < phrases.size() && i < 5; ++i) {
        auto &text = phrases[i];
        if (std::count_if(text.begin(), text.end(), [](unsigned char c) { return (c & 0xc0) != 0x80; }) < 2) continue;
        scored.push_back({{std::move(text), count, 0}, 0, i});
    }
    if (scored.empty()) return {};
    const size_t minStart = count > 6 ? count - 6 : 0;
    for (size_t start = count; start-- > minStart;) {
        if (is_tone(input[start], lexicon.pinyin) || input[start] == '\'') continue;
        const Reading continuation(input.substr(start), lexicon.pinyin);
        const Reading completed(input.substr(start, count - start), lexicon.pinyin);
        if (completed.sounds.empty() || continuation.sounds.size() <= completed.sounds.size()) continue;
        auto bucket = std::lower_bound(lexicon.buckets.begin(), lexicon.buckets.end(), continuation.sounds,
            [](const Lexicon::Bucket &item, std::string_view key) { return item.key < key; });
        for (size_t scanned = 0; bucket != lexicon.buckets.end() && scanned < 96; ++bucket, ++scanned) {
            if (bucket->key.substr(0, continuation.sounds.size()) != continuation.sounds) break;
            if (bucket->key.size() - continuation.sounds.size() > 6) continue;
            const auto range = lexicon.range(bucket->key);
            for (size_t i = range.first; i < range.second && i < range.first + 5; ++i) {
                const auto &word = lexicon.word(i, hans);
                if (word.score(hans) < 4000 || !continuation.matches(word, 0, continuation.sounds.size())) continue;
                const auto reading = word.reading();
                size_t firstTone = 0;
                while (firstTone < reading.size() && !is_tone(reading[firstTone], lexicon.pinyin)) ++firstTone;
                if (firstTone != completed.sounds.size()) continue;
                const auto text = word.text(hans);
                size_t firstEnd = 1;
                while (firstEnd < text.size() && (static_cast<unsigned char>(text[firstEnd]) & 0xc0) == 0x80) ++firstEnd;
                if (firstEnd == text.size()) continue;
                const auto first = text.substr(0, firstEnd);
                for (auto &candidate : scored) {
                    const auto &candidateText = candidate.item.text;
                    if (candidateText.size() >= first.size() && candidateText.compare(candidateText.size() - first.size(), first.size(), first) == 0)
                        candidate.evidence = std::max(candidate.evidence, word.score(hans));
                }
            }
        }
    }
    if (std::none_of(scored.begin(), scored.end(), [](const Scored &item) { return item.evidence > 0; })) return {};
    std::stable_sort(scored.begin(), scored.end(), [](const Scored &a, const Scored &b) {
        return a.evidence == b.evidence ? a.order < b.order : a.evidence > b.evidence;
    });
    std::vector<Fallback> result;
    for (auto &item : scored) result.push_back(std::move(item.item));
    return result;
}

std::vector<Fallback> predictive_segments(const Lexicon &lexicon, std::string_view input, bool hans) {
    if (input.size() < 4 || input.size() > 64) return {};
    for (size_t tail = 1; tail <= std::min(size_t(3), input.size() - 1); ++tail) {
        auto result = predictive_segments_at(lexicon, input, hans, tail);
        if (!result.empty()) return result;
    }
    return {};
}

// A complete pinyin sentence can also be selected in smaller pieces. Only
// offer a split if both sides decode as fully spelled readings.
std::pair<std::vector<Fallback>, size_t> pinyin_segments(const Lexicon &lexicon, std::string_view input, bool hans) {
    if (!lexicon.pinyin || input.size() < 6 || input.size() > 64) return {};
    std::vector<Fallback> result, alternatives;
    size_t inspected = 0;
    const size_t minCut = input.size() > 8 ? input.size() - 8 : 2;
    for (size_t cut = input.size() - 1; cut >= minCut && inspected < 5 && result.size() < 3; --cut) {
        if (is_tone(input[cut], true) || input[cut] == '\'') continue;
        const auto suffix = input.substr(cut);
        if (decode(lexicon, suffix, hans, true).empty()) continue;
        ++inspected;
        auto phrases = decode(lexicon, input.substr(0, cut), hans, true);
        if (phrases.empty()) continue;
        auto &text = phrases.front();
        if (std::count_if(text.begin(), text.end(), [](unsigned char c) { return (c & 0xc0) != 0x80; }) >= 2) {
            result.push_back({std::move(text), cut, 0});
            for (size_t i = 1; i < std::min(size_t(3), phrases.size()); ++i)
                if (std::count_if(phrases[i].begin(), phrases[i].end(), [](unsigned char c) { return (c & 0xc0) != 0x80; }) >= 2)
                    alternatives.push_back({std::move(phrases[i]), cut, 0});
        }
    }
    // Retain the three split points first, then up to two alternative readings
    // at each. Reuse the same decodes; alternatives remain behind whole paths.
    const size_t primaryCount = result.size();
    for (auto &alternative : alternatives) {
        if (result.size() == 9) break;
        result.push_back(std::move(alternative));
    }
    return {std::move(result), primaryCount};
}

// A short suffix is unfinished only when a recorded single-character reading
// extends it and no recorded single-character reading ends at the suffix.
// Multi-character abbreviation buckets alone are not syllable evidence.
bool incomplete_pinyin_syllable(const Lexicon &lexicon, std::string_view suffix) {
    if (suffix.empty() || suffix.size() > 5) return false;
    const auto found = std::lower_bound(lexicon.singleReadings.begin(), lexicon.singleReadings.end(), suffix);
    return found != lexicon.singleReadings.end() && found->size() > suffix.size() &&
           found->substr(0, suffix.size()) == suffix;
}

// Preserve a sentence built from several dictionary entries while its next
// syllable is incomplete. Selection commits only the completed prefix.
std::vector<Fallback> unfinished_sentence(const Lexicon &lexicon, std::string_view input, bool hans,
                                          bool onsetOnly = false, bool syllableOnly = false, size_t after = 0) {
    if (!lexicon.pinyin || input.size() < 4 || input.size() > max_input) return {};
    for (size_t tail = 1; tail <= std::min(size_t(5), input.size() - 1); ++tail) {
        if (input.size() - tail <= after) break;
        const auto suffix = input.substr(input.size() - tail);
        if (!std::all_of(suffix.begin(), suffix.end(), [](char c) { return c >= 'a' && c <= 'z'; })) continue;
        if (syllableOnly && !incomplete_pinyin_syllable(lexicon, suffix)) continue;
        // Only use unmistakably unfinished initials to outrank existing results.
        // Finals such as n/ng may complete an otherwise valid whole sentence.
        if (onsetOnly) {
            constexpr std::string_view initials[] = {"b", "p", "f", "d", "t", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r", "z", "c", "s", "y", "w", "n"};
            if (std::find(std::begin(initials), std::end(initials), suffix) == std::end(initials)) continue;
            if (suffix == "n" && (input.size() < 2 || input[input.size() - 2] != 'n')) continue;
        }
        const auto possible = std::lower_bound(lexicon.buckets.begin(), lexicon.buckets.end(), suffix,
            [](const Lexicon::Bucket &bucket, std::string_view value) { return bucket.key < value; });
        if (possible == lexicon.buckets.end() || possible->key.substr(0, suffix.size()) != suffix) continue;
        const size_t count = input.size() - tail;
        auto phrases = decode(lexicon, input.substr(0, count), hans, true);
        std::vector<Fallback> result;
        for (const auto &phrase : phrases) {
            const auto characters = std::count_if(phrase.begin(), phrase.end(), [](unsigned char c) { return (c & 0xc0) != 0x80; });
            if (characters >= (onsetOnly ? 3 : 2)) result.push_back({phrase, count, 0});
            if (result.size() == 3) break;
        }
        if (!result.empty()) return result;
    }
    return {};
}

// Offer completed Zhuyin prefixes while keeping all following raw keys intact.
// Explicit tones are reliable syllable boundaries; when tones are omitted, a
// few additional boundaries before an initial can be decoded from the lexicon.
std::vector<Fallback> zhuyin_segments(const Lexicon &lexicon, std::string_view input, bool hans) {
    if (lexicon.pinyin || input.size() < 4) return {};
    std::vector<Fallback> result, alternatives;
    auto collect = [&](size_t count) {
        auto phrases = decode(lexicon, input.substr(0, count), hans);
        if (phrases.empty()) return;
        const bool newest = result.empty();
        auto &phrase = phrases.front();
        const auto characters = std::count_if(phrase.begin(), phrase.end(), [](unsigned char c) { return (c & 0xc0) != 0x80; });
        if (characters >= 2) {
            result.push_back({std::move(phrase), count, 0});
            if (newest)
                for (size_t i = 1; i < std::min(size_t(3), phrases.size()); ++i)
                    if (std::count_if(phrases[i].begin(), phrases[i].end(), [](unsigned char c) { return (c & 0xc0) != 0x80; }) >= 2)
                        alternatives.push_back({std::move(phrases[i]), count, 0});
        }
    };
    // Decode at most 64 leading keys even when the composition is longer.
    // Returning no segments past that limit made long sentences fall back to
    // single characters. Selecting a bounded prefix leaves all later keys intact.
    const size_t upper = std::min(input.size() - 1, size_t(64));
    const bool longComposition = input.size() > 64;
    size_t inspected = 0;
    for (size_t count = upper; count > 0 && inspected < (longComposition ? 2 : 6); --count) {
        if (!is_tone(input[count - 1]) || is_tone(input[count]) || input[count] == '\'') continue;
        ++inspected;
        collect(count);
    }
    // Bound the additional search and avoid splitting immediately before a
    // tone, which belongs to the preceding syllable.
    inspected = 0;
    constexpr auto initials = keys.substr(0, 21);
    for (size_t count = upper; count >= 4 && inspected < (longComposition ? 2 : 4) && result.size() < 6; --count) {
        if (is_tone(input[count - 1]) || initials.find(input[count]) == initials.npos) continue;
        ++inspected;
        collect(count);
    }
    for (auto &item : alternatives) result.push_back(std::move(item));
    return result;
}
} // namespace

struct MPNLexicon { std::shared_ptr<const Lexicon> value; };
struct MPNSession {
    struct JapaneseWord { std::string reading, text; int weight; };
    std::shared_ptr<const Lexicon> lexicon;
    std::string input, preedit, committed, mixedPinyin, segmentContext, exactPinyin;
    std::vector<JapaneseWord> japanese;
    std::vector<Candidate> candidates;
    size_t page = 0;
    bool hans = false;
    bool requireFullCandidate = false;
    size_t sentenceCount = 0, literalTail = 0;

    void reset() {
        input.clear();
        preedit.clear();
        candidates.clear();
        segmentContext.clear();
        sentenceCount = literalTail = 0;
        page = 0;
    }

    // While a recorded Japanese reading is still incomplete, keep the
    // preceding fully decoded Chinese phrase selectable. Never guess or
    // consume the unfinished Japanese keys; selecting a phrase leaves them
    // in the session for subsequent input.
    std::vector<Fallback> unfinishedJapanesePrefix(bool allowOne) const {
        if (!lexicon->pinyin || input.size() < 5 || input.size() > max_input || japanese.empty()) return {};
        const auto maxTail = std::min(size_t(16), input.size() - 2);
        for (size_t tail = maxTail; tail >= (allowOne ? 1u : 2u); --tail) {
            const auto suffix = std::string_view(input).substr(input.size() - tail);
            const bool hasReading = std::any_of(japanese.begin(), japanese.end(), [&](const JapaneseWord &word) {
                return word.weight >= 4000 && word.reading.size() > tail &&
                       std::string_view(word.reading).substr(0, tail) == suffix;
            });
            if (!hasReading) continue;
            const size_t cut = input.size() - tail;
            auto phrases = decode(*lexicon, std::string_view(input).substr(0, cut), hans, true);
            std::vector<Fallback> result;
            for (auto &phrase : phrases) {
                const auto characters = std::count_if(phrase.begin(), phrase.end(),
                    [](unsigned char c) { return (c & 0xc0) != 0x80; });
                if (characters >= 2) result.push_back({std::move(phrase), cut, 0});
                if (result.size() == 3) break;
            }
            if (!result.empty()) return result;
        }
        return {};
    }

    void refresh() {
        std::string marked;
        if (lexicon->pinyin) marked = input;
        else for (char key : input) marked += symbols[keys.find(key)];
        std::vector<CandidateMetadata> metadata;
        bool singleReading = false;
        auto decoded = decode(*lexicon, input, hans, false, &metadata, &singleReading);
        std::vector<Candidate> ranked;
        ranked.reserve(decoded.size());
        for (size_t i = 0; i < decoded.size(); ++i)
            ranked.push_back({std::move(decoded[i]), input.size(), metadata[i]});
        const auto ranked_limit = singleReading ? single_character_candidate_limit : candidate_limit;
        const auto sameSelection = [](const Candidate &candidate, const Fallback &item) {
            return candidate.text == item.text && candidate.consumed == item.consumed;
        };
        const auto partialCandidate = [](Fallback &item) {
            return Candidate{std::move(item.text), item.consumed, {MPN_CANDIDATE_PARTIAL, MPN_SCORE_NONE, 0}};
        };
        // Several ranking branches need the same existence check. Decode it
        // once per refresh, lazily, rather than rebuilding identical beams.
        int completeReading = -1;
        const auto hasCompleteReading = [&]() {
            if (completeReading < 0) completeReading = !decode(*lexicon, input, hans, true).empty();
            return completeReading != 0;
        };
        auto partial = unfinished_phrase(*lexicon, input, hans);
        auto prepend = [&](std::vector<Fallback> &items) {
          for (auto it = items.rbegin(); it != items.rend(); ++it) {
            const auto duplicate = std::find_if(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return sameSelection(candidate, *it); });
            if (duplicate != ranked.end()) {
                ranked.erase(duplicate);
            }
            ranked.insert(ranked.begin(), partialCandidate(*it));
            if (ranked.size() > ranked_limit) ranked.pop_back();
          }
        };
        prepend(partial);
        auto predictive = predictive_segments(*lexicon, input, hans);
        if (!predictive.empty()) {
            const bool noComplete = lexicon->pinyin ? !hasCompleteReading() : ranked.empty();
            if (noComplete) prepend(predictive);
            else {
                // An untoned prefix can also form a valid whole sentence.
                // Keep the first three complete interpretations ahead of it.
                size_t position = std::min(size_t(3), ranked.size());
                for (auto &item : predictive) {
                    if (std::any_of(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return sameSelection(candidate, item); })) continue;
                    ranked.insert(ranked.begin() + position, partialCandidate(item));
                    ++position;
                    if (ranked.size() > ranked_limit) ranked.pop_back();
                }
            }
        }
        if (!partial.empty()) {
            // A shorter dictionary word must not prevent a longer decoded
            // sentence prefix from remaining selectable. Search only beyond
            // the prefix already present, and only for an unfinished syllable.
            const size_t consumed = std::max(partial.front().consumed,
                predictive.empty() ? size_t(0) : predictive.front().consumed);
            if (consumed + 1 < input.size() && !hasCompleteReading()) {
                auto longer = unfinished_sentence(*lexicon, input, hans, false, true, consumed);
                prepend(longer);
            }
        }
        if (!ranked.empty() && partial.empty() && predictive.empty() && lexicon->pinyin && input.size() >= 6 && input.size() <= max_input &&
            std::string_view("bpftdlgkhjqxzcrsywn").find(input.back()) != std::string_view::npos &&
            (input.back() != 'n' || input[input.size() - 2] == 'n') &&
            !has_exact_match(*lexicon, input) &&
            // Beyond 64 keys, avoid a second full-sentence decode on every
            // key. Final ng and er can complete a pinyin syllable; other
            // consonant initials here are necessarily incomplete.
            (input.size() <= 64 ? !hasCompleteReading() :
                !((input.back() == 'g' && input[input.size() - 2] == 'n') ||
                  (input.back() == 'r' && input[input.size() - 2] == 'e')))) {
            auto sentence = unfinished_sentence(*lexicon, input, hans, true);
            prepend(sentence);
        }
        if (ranked.empty()) {
            for (auto &item : unfinished_sentence(*lexicon, input, hans)) {
                ranked.push_back(partialCandidate(item));
            }
        }
        if (lexicon->pinyin && !ranked.empty() && partial.empty() && predictive.empty() && input.size() >= 6 &&
            std::none_of(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return candidate.consumed < input.size(); }) &&
            !has_exact_match(*lexicon, input)) {
            // Do not let abbreviation chains such as 狄歐尼 hide a completed
            // Chinese phrase while its next real pinyin syllable is unfinished.
            auto sentence = unfinished_sentence(*lexicon, input, hans, false, true);
            if (!sentence.empty() && !hasCompleteReading()) prepend(sentence);
        }
        if (lexicon->pinyin && !japanese.empty() && input.size() >= 5 && input.size() <= max_input &&
            !has_exact_match(*lexicon, input)) {
            // A one-letter Japanese onset is ambiguous; offer its preceding
            // phrase only when no Chinese result can otherwise be decoded.
            auto segments = unfinishedJapanesePrefix(ranked.empty());
            // Preserve the best existing Chinese interpretations while making
            // a completed phrase available on the first page. An identical
            // existing candidate keeps its original consumed-key semantics.
            size_t position = std::min(size_t(3), ranked.size());
            for (auto &item : segments) {
                const auto existing = std::find_if(ranked.begin(), ranked.end(), [&](const Candidate &candidate) {
                    return sameSelection(candidate, item);
                });
                if (existing != ranked.end()) {
                    const auto index = static_cast<size_t>(existing - ranked.begin());
                    if (index < position) continue;
                    auto candidate = std::move(ranked[index]);
                    ranked.erase(ranked.begin() + index);
                    ranked.insert(ranked.begin() + position, std::move(candidate));
                } else {
                    ranked.insert(ranked.begin() + position, partialCandidate(item));
                }
                ++position;
                if (ranked.size() > ranked_limit) ranked.pop_back();
            }
        }
        if (!lexicon->pinyin) {
            auto segments = zhuyin_segments(*lexicon, input, hans);
            // Preserve the best whole-sentence alternatives. Place up to
            // three segments on the first page and the rest on later pages.
            size_t position = std::min(size_t(3), ranked.size());
            size_t inserted = 0;
            for (auto &segment : segments) {
                // Keep an exact full-input candidate's selection semantics.
                if (std::any_of(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return sameSelection(candidate, segment); })) continue;
                if (inserted == 3) position = std::min(size_t(10), ranked.size());
                ranked.insert(ranked.begin() + position, partialCandidate(segment));
                ++position;
                ++inserted;
                if (ranked.size() > ranked_limit) ranked.pop_back();
            }
        }
        if (lexicon->pinyin && !ranked.empty()) {
            size_t completed = input.size();
            if (!hasCompleteReading()) {
                completed = 0;
                for (const auto &candidate : ranked)
                    if (candidate.consumed < input.size()) completed = std::max(completed, candidate.consumed);
            }
            // An unfinished following word must not remove the earlier split
            // points of a completed phrase. The existing bounded segmenter
            // consumes only keys within that prefix, leaving the entire tail.
            auto [segments, primaryCount] = pinyin_segments(*lexicon, std::string_view(input).substr(0, completed), hans);
            // Keep the existing leading split choices. Append extra homophones
            // after the original candidates so complete sentences keep their ranks.
            const size_t leadingCount = completed < input.size() ? std::min(size_t(3), segments.size()) : primaryCount;
            size_t position = std::min(size_t(3), ranked.size());
            for (size_t i = 0; i < segments.size(); ++i) {
                auto &segment = segments[i];
                if (std::any_of(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return sameSelection(candidate, segment); })) continue;
                if (i >= leadingCount) position = ranked.size();
                ranked.insert(ranked.begin() + position, partialCandidate(segment));
                ++position;
                if (ranked.size() > ranked_limit) ranked.pop_back();
            }
        }
        // Keep exact shorter choices available when a word or sentence starts
        // with an unwanted homophone, without moving complete interpretations.
        if (input.size() >= 4 && !ranked.empty()) {
            size_t added = 0;
            size_t afterComplete = 0;
            for (size_t i = 0; i < ranked.size(); ++i)
                if (ranked[i].consumed == input.size()) afterComplete = i + 1;
            // Reserve room even when full-reading homophones fill the pool.
            const size_t insertAt = std::min(std::max(size_t(7), afterComplete), ranked_limit - 24);
            for (size_t cut = 1; cut < std::min(input.size(), size_t(13)) && added < 24; ++cut) {
                if (is_tone(input[cut], lexicon->pinyin) || input[cut] == '\'') continue;
                const Reading prefix(std::string_view(input).substr(0, cut), lexicon->pinyin);
                const auto range = lexicon->range(prefix.sounds);
                size_t perCut = 0;
                for (size_t i = range.first; i < range.second && i - range.first < 48 && added < 24; ++i) {
                    const auto &word = lexicon->word(i, hans);
                    const auto text = word.text(hans);
                    if ((lexicon->pinyin && !fully_spelled(word)) || !prefix.matches(word, 0, prefix.sounds.size()) || han_characters(text).empty()) continue;
                    Fallback item{std::string(text), cut, double(word.score(hans))};
                    if (std::any_of(ranked.begin(), ranked.end(), [&](const Candidate &candidate) { return sameSelection(candidate, item); })) continue;
                    const auto position = std::min(insertAt + added, ranked.size());
                    ranked.insert(ranked.begin() + position, partialCandidate(item));
                    if (ranked.size() > ranked_limit) ranked.pop_back();
                    ++added;
                    if (++perCut == 4) break;
                }
            }
        }
        if (ranked.empty()) {
            for (auto &item : fallback(*lexicon, input, hans)) {
                ranked.push_back({std::move(item.text), item.consumed,
                    {item.consumed < input.size() ? MPN_CANDIDATE_PARTIAL : MPN_CANDIDATE_CORRECTION,
                     MPN_SCORE_FALLBACK, item.score}});
            }
        }
        if (!segmentContext.empty() && ranked.size() > 1) {
            // Only promote a strong recorded phrase within a bounded candidate
            // window. The original index supplies a modest prior; no text or
            // consumed-key lengths are changed by this local reordering.
            const size_t limit = std::min(size_t(64), ranked.size());
            struct RankedIndex { size_t index; double score; };
            // Context may reorder homophones for the same selection span,
            // but must not move a shorter prefix over an existing whole path.
            std::unordered_map<size_t, std::vector<RankedIndex>> groups;
            const bool exact = has_exact_match(*lexicon, input);
            for (size_t i = 0; i < limit; ++i) {
                const auto traditional = lexicon->traditionalize(ranked[i].text, false);
                const double bonus = traditional.empty() || (exact && ranked[i].consumed != input.size())
                    ? 0 : lexicon->join_bonus(segmentContext, traditional, hans, true);
                groups[ranked[i].consumed].push_back({i, -static_cast<double>(i) * 0.18 + (bonus >= 2.5 ? bonus : 0)});
            }
            std::vector<Candidate> ordered(limit);
            for (const auto &group : groups) {
                auto order = group.second;
                std::stable_sort(order.begin(), order.end(), [](const RankedIndex &a, const RankedIndex &b) { return a.score > b.score; });
                for (size_t i = 0; i < order.size(); ++i)
                    ordered[group.second[i].index] = std::move(ranked[order[i].index]);
            }
            for (size_t i = 0; i < limit; ++i) ranked[i] = std::move(ordered[i]);
        }
        if (requireFullCandidate && lexicon->pinyin && !input.empty()) {
            // The Android presentation needs complete interpretations first,
            // followed by selectable prefixes. Never compare scores from
            // different decoder paths or consume a displayed raw suffix.
            std::vector<Candidate> complete, rest;
            for (auto &candidate : ranked) {
                const bool whole = candidate.consumed == input.size() &&
                    candidate.metadata.kind != MPN_CANDIDATE_CORRECTION &&
                    candidate.metadata.kind != MPN_CANDIDATE_PARTIAL &&
                    candidate.metadata.scoreKind != MPN_SCORE_FALLBACK;
                if (whole && complete.size() < 3) complete.push_back(std::move(candidate));
                else rest.push_back(std::move(candidate));
            }
            literalTail = 0;
            if (complete.empty()) {
                auto prefix = std::find_if(rest.begin(), rest.end(), [&](const Candidate &candidate) {
                    return candidate.consumed > 0 && candidate.consumed < input.size();
                });
                if (prefix != rest.end()) {
                    auto sentence = *prefix;
                    sentence.literalTail = input.size() - sentence.consumed;
                    sentence.text += input.substr(sentence.consumed);
                    literalTail = sentence.literalTail;
                    complete.push_back(std::move(sentence));
                } else {
                    literalTail = input.size();
                    complete.push_back({input, input.size(), {MPN_CANDIDATE_UNKNOWN, MPN_SCORE_NONE, 0}, 0});
                }
            }
            sentenceCount = complete.size();
            complete.insert(complete.end(), std::make_move_iterator(rest.begin()), std::make_move_iterator(rest.end()));
            if (std::none_of(complete.begin(), complete.end(), [&](const Candidate &candidate) {
                return candidate.text == input && candidate.consumed == input.size();
            })) complete.push_back({input, input.size(), {MPN_CANDIDATE_UNKNOWN, MPN_SCORE_NONE, 0}, 0});
            ranked = std::move(complete);
            if (ranked.size() > ranked_limit) ranked.resize(ranked_limit);
        } else {
            sentenceCount = literalTail = 0;
        }
        preedit.swap(marked);
        candidates.swap(ranked);
        page = 0;
    }
};

MPNLexicon *mpn_lexicon_create(const char *data, size_t size) {
    if (!data || !size) return nullptr;
    try {
        auto lexicon = std::make_shared<Lexicon>();
        if (!lexicon->load({data, size})) return nullptr;
        return new MPNLexicon{std::move(lexicon)};
    } catch (...) { return nullptr; }
}
void mpn_lexicon_destroy(MPNLexicon *lexicon) { delete lexicon; }

MPNSession *mpn_session_create(const MPNLexicon *lexicon) {
    if (!lexicon) return nullptr;
    try {
        auto session = std::make_unique<MPNSession>();
        session->lexicon = lexicon->value;
        return session.release();
    } catch (...) { return nullptr; }
}
void mpn_session_destroy(MPNSession *session) { delete session; }

int mpn_add_japanese_word(MPNSession *session, const char *reading, const char *text, int weight) {
    // 3500 is reserved for pinyin aliases of all-kanji Japanese words. They
    // require another independently recognised Japanese fragment to split a sentence.
    if (!session || !reading || !text || (weight < 4000 && weight != 3500) || session->japanese.size() >= 512) return 0;
    const std::string_view key(reading), value(text);
    // Most two-letter readings collide with common Chinese syllables. "au"
    // (会う) is explicitly supported; it is not a valid standalone pinyin
    // syllable, unlike e.g. "ai" or "su".
    if ((key.size() < 3 && !(key == "au" && weight >= 4000)) || key.size() > 20 || value.empty() || value.size() > 128 ||
        !std::all_of(key.begin(), key.end(), [](char c) { return c >= 'a' && c <= 'z'; })) return 0;
    try { session->japanese.push_back({std::string(key), std::string(value), weight}); return 1; }
    catch (...) { return 0; }
}

const char *mpn_mixed_pinyin(MPNSession *session, const char *reading, int simplified) {
    if (!session) return "";
    session->mixedPinyin.clear();
    if (!reading || !session->lexicon->pinyin) return session->mixedPinyin.c_str();
    const std::string_view input(reading);
    if (input.size() < 5 || input.size() > 80 || !std::all_of(input.begin(), input.end(), [](char c) { return c >= 'a' && c <= 'z'; }))
        return session->mixedPinyin.c_str();
    try {
        struct Match { size_t start, end; const MPNSession::JapaneseWord *word; };
        std::vector<Match> matches;
        auto precedes = [](const Match &a, const Match &b) {
            if (a.start != b.start) return a.start < b.start;
            if (a.end != b.end) return a.end > b.end;
            return a.word->weight > b.word->weight;
        };
        // Keep a bounded shortlist even for adversarial repeated input and
        // duplicate readings: do not allocate/sort every possible occurrence.
        constexpr size_t matchShortlistLimit = 32;
        for (const auto &word : session->japanese) {
            for (size_t pos = input.find(word.reading); pos != input.npos; pos = input.find(word.reading, pos + 1)) {
                if (pos == 0 && word.reading.size() == input.size()) continue;
                const Match match{pos, pos + word.reading.size(), &word};
                if (matches.size() == matchShortlistLimit && !precedes(match, matches.back())) break;
                const auto at = std::lower_bound(matches.begin(), matches.end(), match, precedes);
                matches.insert(at, match);
                if (matches.size() > matchShortlistLimit) matches.pop_back();
            }
        }
        // Prefer a longer recorded form over splitting its own shorter form
        // into Japanese plus accidental Chinese fragments (推し / 推し活).
        std::vector<Match> compact;
        for (const auto &match : matches) {
            const bool covered = std::any_of(compact.begin(), compact.end(), [&](const Match &other) {
                return other.start == match.start && other.end > match.end &&
                    other.word->text.compare(0, match.word->text.size(), match.word->text) == 0;
            });
            if (!covered) compact.push_back(match);
            if (compact.size() == 16) break;
        }
        matches = std::move(compact);
        size_t sentenceDecodes = 0;
        constexpr size_t sentenceDecodeLimit = 12;
        using Chinese = std::pair<std::string, int>;
        std::unordered_map<std::string, Chinese> cache;
        auto chinese = [&](std::string_view part) -> Chinese {
            if (part.empty()) return {"", 0};
            const std::string key(part);
            if (auto it = cache.find(key); it != cache.end()) return it->second;
            Chinese result;
            const Reading parsed(part, true);
            const auto range = session->lexicon->range(parsed.sounds);
            for (size_t i = range.first; i < range.second; ++i) {
                const auto &word = session->lexicon->word(i, simplified != 0);
                if (word.score(simplified != 0) < 2000) break;
                if (parsed.matches(word, 0, parsed.sounds.size())) { result = {std::string(word.text(simplified != 0)), word.score(simplified != 0)}; break; }
            }
            if (result.first.empty() && sentenceDecodes < sentenceDecodeLimit) {
                ++sentenceDecodes;
                auto sentences = decode(*session->lexicon, part, simplified != 0, true);
                if (!sentences.empty()) result = {std::move(sentences.front()), 2000};
            }
            cache.emplace(key, result);
            return result;
        };
        int64_t bestScore = -1;
        auto consider = [&](const Match &first, const Match *second, const Match *third) {
            if (first.word->weight == 3500 && (!second || second->word->weight == 3500) &&
                (!third || third->word->weight == 3500)) return;
            const auto end = third ? third->end : second ? second->end : first.end;
            if (first.start == 0 && end == input.size() &&
                (!second || first.end == second->start) && (!third || second->end == third->start)) return;
            const auto before = chinese(input.substr(0, first.start));
            const auto middle = second ? chinese(input.substr(first.end, second->start - first.end)) : Chinese{"", 0};
            const auto middle2 = third ? chinese(input.substr(second->end, third->start - second->end)) : Chinese{"", 0};
            const auto after = chinese(input.substr(end));
            if ((first.start && before.first.empty()) || (second && first.end != second->start && middle.first.empty()) ||
                (third && second->end != third->start && middle2.first.empty()) ||
                (end < input.size() && after.first.empty())) return;
            // Short readings are much more likely to occur incidentally inside
            // English or Chinese. Require an independent Japanese fragment or
            // an exact, high-weight Chinese word with at least two characters.
            const bool shortReading = first.word->reading.size() == 2 ||
                (second && second->word->reading.size() == 2) || (third && third->word->reading.size() == 2);
            if (shortReading) {
                const auto strongChinese = [](const Chinese &part) {
                    const auto length = std::count_if(part.first.begin(), part.first.end(),
                        [](unsigned char c) { return (c & 0xc0) != 0x80; });
                    return part.second >= 4000 && length >= 2;
                };
                const auto strongJapanese = [](const Match *part) {
                    return part && part->word->reading.size() >= 3 && part->word->weight >= 4000;
                };
                if (!strongChinese(before) && !strongChinese(middle) && !strongChinese(middle2) &&
                    !strongChinese(after) && !strongJapanese(&first) && !strongJapanese(second) && !strongJapanese(third)) return;
            }
            // Prefer a longer recorded Japanese word over splitting its prefix
            // into Japanese plus accidental low-quality Chinese fragments.
            const int japaneseLength = int(first.end - first.start + (second ? second->end - second->start : 0) +
                                           (third ? third->end - third->start : 0));
            // A single-kanji Japanese reading may also be valid pinyin. If
            // a stronger Japanese word follows immediately and the preceding
            // Chinese phrase can absorb this syllable, avoid rewarding an
            // accidental extra Japanese split over the complete Chinese.
            const auto ambiguousHanBeforeJapanese = [&](const Match *part) {
                if (!part || part->start == 0 || han_characters(part->word->text).size() != 1 ||
                    !has_exact_match(*session->lexicon, part->word->reading)) return false;
                const bool strongerFollowing = std::any_of(matches.begin(), matches.end(), [&](const Match &other) {
                    return other.start == part->end && other.word->weight >= 4000 &&
                           other.word->reading.size() >= 4;
                });
                if (!strongerFollowing) return false;
                const auto merged = chinese(input.substr(0, part->end));
                return han_characters(merged.first).size() >= 2;
            };
            // The same ambiguity can occur *after* a known Japanese word:
            // a trailing Chinese syllable may also be a Japanese one-Han
            // reading. Prefer a coherent Chinese phrase following the known
            // Japanese word over inventing another Japanese fragment.
            const auto ambiguousHanAfterJapanese = [&](const Match *part) {
                if (!part || han_characters(part->word->text).size() != 1 ||
                    !has_exact_match(*session->lexicon, part->word->reading)) return false;
                for (const Match *previous : {&first, second, third}) {
                    if (!previous || previous == part || previous->end >= part->start ||
                        previous->word->weight < 4000 || previous->word->reading.size() < 4) continue;
                    const auto merged = chinese(input.substr(previous->end, part->end - previous->end));
                    const auto traditional = session->lexicon->traditionalize(merged.first, false);
                    const auto characters = han_characters(traditional);
                    if (characters.size() < 2) continue;
                    const auto last = characters.back();
                    const auto head = std::string_view(traditional).substr(0, traditional.size() - last.size());
                    if (merged.second >= 4000 || session->lexicon->join_bonus(head, last, simplified != 0) > 0) return true;
                }
                return false;
            };
            const int ambiguityPenalty = 24000 * (int(ambiguousHanBeforeJapanese(&first)) +
                int(ambiguousHanBeforeJapanese(second)) + int(ambiguousHanBeforeJapanese(third)) +
                int(ambiguousHanAfterJapanese(second)) + int(ambiguousHanAfterJapanese(third)));
            // Both dictionaries accept non-negative int weights. Accumulate
            // several fragments in 64 bits so valid large weights cannot wrap.
            const int64_t score = static_cast<int64_t>(before.second) + middle.second + middle2.second + after.second + first.word->weight +
                              (second ? second->word->weight : 0) + (third ? third->word->weight : 0) + japaneseLength * 1000 - ambiguityPenalty;
            if (score > bestScore) {
                bestScore = score;
                session->mixedPinyin = before.first + first.word->text + middle.first + (second ? second->word->text : "") +
                                      middle2.first + (third ? third->word->text : "") + after.first;
            }
        };
        size_t combinations = 0;
        constexpr size_t combinationLimit = 256;
        for (size_t i = 0; i < matches.size() && combinations < combinationLimit; ++i) {
            ++combinations;
            consider(matches[i], nullptr, nullptr);
            for (size_t j = i + 1; j < matches.size() && combinations < combinationLimit; ++j) {
                if (matches[j].start < matches[i].end) continue;
                ++combinations;
                consider(matches[i], &matches[j], nullptr);
                for (size_t k = j + 1; k < matches.size() && combinations < combinationLimit; ++k) {
                    if (matches[k].start < matches[j].end) continue;
                    ++combinations;
                    consider(matches[i], &matches[j], &matches[k]);
                }
            }
        }
    } catch (...) { session->mixedPinyin.clear(); }
    return session->mixedPinyin.c_str();
}

const char *mpn_mixed_pinyin_prefix(MPNSession *session, const char *reading, int simplified, size_t *consumed) {
    if (consumed) *consumed = 0;
    if (!session) return "";
    session->mixedPinyin.clear();
    if (!reading || !session->lexicon->pinyin) return session->mixedPinyin.c_str();
    const std::string_view inputView(reading);
    if (inputView.size() < 4 || inputView.size() > max_input || !std::all_of(inputView.begin(), inputView.end(), [](char c) { return c >= 'a' && c <= 'z'; })) return session->mixedPinyin.c_str();
    try {
        // Keep prefix selection available beyond the whole-sentence decoder's
        // bounded window. The reported byte count belongs to the original
        // input, so the entire unconverted suffix remains in the session.
        if (inputView.size() > 80) {
            const std::string window(inputView.substr(0, 80));
            return mpn_mixed_pinyin_prefix(session, window.c_str(), simplified, consumed);
        }
        const std::string input(inputView);
        // Preserve the longest already-decoded mixed phrase when the next
        // Chinese syllable is still an unfinished initial. This avoids
        // reducing 我好き你h to 我好き while keeping the h unconsumed.
        if (input.size() >= 6) {
            constexpr std::string_view initials[] = {"b", "p", "f", "d", "t", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r", "z", "c", "s", "y", "w", "n"};
            for (size_t tail = 1; tail <= std::min(size_t(2), input.size() - 4); ++tail) {
                const auto suffix = std::string_view(input).substr(input.size() - tail);
                if (std::find(std::begin(initials), std::end(initials), suffix) == std::end(initials)) continue;
                if (tail == 1 && ((suffix == "n" && input[input.size() - 2] != 'n') ||
                                  (suffix == "g" && input[input.size() - 2] == 'n') ||
                                  (suffix == "r" && input[input.size() - 2] == 'e'))) continue;
                const bool completeJapanese = std::any_of(session->japanese.begin(), session->japanese.end(), [&](const auto &word) {
                    return word.reading.size() <= input.size() &&
                           std::string_view(input).substr(input.size() - word.reading.size()) == word.reading;
                });
                if (completeJapanese) break;
                const auto candidate = input.substr(0, input.size() - tail);
                const std::string result(mpn_mixed_pinyin(session, candidate.c_str(), simplified));
                if (!result.empty()) {
                    session->mixedPinyin = result;
                    if (consumed) *consumed = candidate.size();
                    return session->mixedPinyin.c_str();
                }
            }
        }
        // Only complete recorded Japanese words can end a prefix. Shortest
        // first gives the user a useful partial choice without eating a suffix.
        size_t attempts = 0;
        for (size_t end = 2; end < input.size(); ++end) {
            for (const auto &word : session->japanese) {
                if (word.reading.size() > end || input.compare(end - word.reading.size(), word.reading.size(), word.reading) != 0) continue;
                const size_t start = end - word.reading.size();
                std::string result;
                if (start == 0) {
                    if (word.weight == 3500) {
                        // An ambiguous all-Han alias may be selected as a
                        // leading segment only when the unselected suffix
                        // independently contains a known Japanese reading.
                        const auto suffix = std::string_view(input).substr(end);
                        const bool hasJapaneseEvidence = std::any_of(session->japanese.begin(), session->japanese.end(),
                            [&](const MPNSession::JapaneseWord &other) {
                                return other.weight >= 4000 && other.reading.size() >= 3 &&
                                    suffix.find(other.reading) != std::string_view::npos;
                            });
                        if (!hasJapaneseEvidence) continue;
                    }
                    if ((word.weight == 3500 || word.reading.size() == 2) &&
                        std::string_view(mpn_mixed_pinyin(session, input.c_str(), simplified)).empty()) continue;
                    result = word.text;
                } else {
                    if (++attempts > 8) return session->mixedPinyin.c_str();
                    const auto prefix = input.substr(0, end);
                    // Do not replace a complete Chinese phrase with an
                    // incidental Japanese homophone inside that phrase.
                    if (!decode(*session->lexicon, prefix, simplified != 0, true).empty()) continue;
                    result = mpn_mixed_pinyin(session, prefix.c_str(), simplified);
                }
                if (!result.empty()) {
                    session->mixedPinyin = std::move(result);
                    if (consumed) *consumed = end;
                    return session->mixedPinyin.c_str();
                }
            }
        }
    } catch (...) {}
    session->mixedPinyin.clear();
    return session->mixedPinyin.c_str();
}

void mpn_set_simplified(MPNSession *session, int simplified) {
    if (session->hans == (simplified != 0)) return;
    session->hans = simplified != 0;
    try { session->refresh(); } catch (...) { session->reset(); }
}
void mpn_clear(MPNSession *session) {
    session->reset();
    session->committed.clear();
}
void mpn_require_full_candidate(MPNSession *session) {
    if (!session) return;
    session->requireFullCandidate = true;
    try { session->refresh(); } catch (...) { session->reset(); }
}
void mpn_set_segment_context(MPNSession *session, const char *text) {
    if (!session) return;
    try {
        const std::string_view input = text ? std::string_view(text) : std::string_view();
        session->segmentContext = input.size() <= 64 ? session->lexicon->traditionalize(input, true) : std::string();
        if (!session->input.empty()) session->refresh();
    } catch (...) { session->segmentContext.clear(); }
}
const char *mpn_segment_context(const MPNSession *session) { return session ? session->segmentContext.c_str() : ""; }
void mpn_commit(MPNSession *session) {
    if (session->input.empty()) return;
    if (!session->candidates.empty()) { mpn_choose(session, session->page * page_size); return; }
    // Preserve what the user sees: Zhuyin's physical keys differ from its
    // displayed symbols, whereas Pinyin preedit is the original reading.
    session->committed += session->preedit;
    session->reset();
}
int mpn_choose(MPNSession *session, size_t index) {
    if (index >= session->candidates.size()) return 0;
    auto text = session->candidates[index].text;
    if (const auto tail = session->candidates[index].literalTail; tail && tail <= text.size())
        text.resize(text.size() - tail);
    auto remaining = session->input.substr(session->candidates[index].consumed);
    const auto traditional = session->lexicon->traditionalize(text, true);
    auto context = traditional.empty() ? std::string() : session->lexicon->traditionalize(session->segmentContext + traditional, true);
    session->reset();
    session->input = std::move(remaining);
    if (!session->input.empty()) session->segmentContext = std::move(context);
    session->refresh();
    session->committed += text;
    return 1;
}

size_t mpn_max_input_bytes(void) { return max_input; }

int mpn_key(MPNSession *session, int32_t key) {
    try {
        auto &input = session->input;
        if (key == 0xff55 || key == 0xff56) {
            if (input.empty()) return 0;
            if (key == 0xff55 && session->page > 0) --session->page;
            if (key == 0xff56 && (session->page + 1) * page_size < session->candidates.size()) ++session->page;
            return 1;
        }
        // Cancelling the active composition must not discard an earlier
        // selection that the host has not taken yet.
        if (key == 0xff1b && !input.empty()) { session->reset(); return 1; }
        if (key == 0xff08 && !input.empty()) {
            input.pop_back();
            if (input.empty()) session->segmentContext.clear();
            session->refresh();
            return 1;
        }
        if (key == 0xff0d && !input.empty()) { mpn_commit(session); return 1; }
        if (key < 32 || key > 126) return 0;
        const auto ch = static_cast<char>(key);
        if (session->lexicon->pinyin) {
            // At the bounded composition limit, preserve the visible reading
            // as entered instead of silently dropping the next key. A space
            // finishes the literal reading and remains an actual space.
            if (ch == ' ' && input.size() == max_input) {
                session->committed += session->preedit;
                session->committed += ' ';
                session->reset();
                return 1;
            }
            if (ch == ' ' && !input.empty()) { mpn_commit(session); return 1; }
            const bool boundary = is_tone(ch, true) || ch == '\'';
            if (!(ch >= 'a' && ch <= 'z') && !boundary) return 0;
            if (boundary && (input.empty() || is_tone(input.back(), true) || input.back() == '\'')) return 0;
            if (input.size() == max_input) {
                session->committed += session->preedit;
                // A tone/separator belongs to the reading before it; do not
                // leave an invalid orphaned boundary in the new composition.
                if (boundary) session->committed += ch;
                session->reset();
                if (boundary) return 1;
            }
            input += ch; session->refresh(); return 1;
        }
        const bool boundary = is_tone(ch) || ch == '\'';
        if (keys.find(ch) == keys.npos || (boundary && input.empty())) return 0;
        if (boundary && (is_tone(input.back()) || input.back() == '\'')) {
            if (ch == ' ') mpn_commit(session);
            return 1;
        }
        if (input.size() == max_input) {
            session->committed += session->preedit;
            // The tone symbol completes the previous syllable, rather than
            // becoming an orphaned new composition after the rollover.
            if (boundary) session->committed += symbols[keys.find(ch)];
            session->reset();
            if (boundary) return 1;
        }
        input += ch;
        session->refresh();
        return 1;
    } catch (...) {
        session->reset();
        return 0;
    }
}

const char *mpn_input(const MPNSession *session) { return session->input.c_str(); }
const char *mpn_preedit(const MPNSession *session) { return session->preedit.c_str(); }
const char *mpn_committed(const MPNSession *session) { return session->committed.c_str(); }
void mpn_clear_commit(MPNSession *session) { session->committed.clear(); }
size_t mpn_candidate_count(const MPNSession *session) { return session->candidates.size(); }
const char *mpn_candidate(const MPNSession *session, size_t index) {
    return session && index < session->candidates.size() ? session->candidates[index].text.c_str() : "";
}
size_t mpn_candidate_consumed(const MPNSession *session, size_t index) {
    return session && index < session->candidates.size() ? session->candidates[index].consumed : 0;
}
int mpn_candidate_info(const MPNSession *session, size_t index, MPNCandidateInfo *info, size_t info_size) {
    if (!session || !info || !info_size || index >= session->candidates.size()) return 0;
    const auto &candidate = session->candidates[index];
    const MPNCandidateInfo value{candidate.metadata.kind, candidate.metadata.scoreKind,
        static_cast<uint64_t>(candidate.consumed), static_cast<uint64_t>(candidate.consumed), candidate.metadata.score};
    std::memcpy(info, &value, std::min(info_size, sizeof(value)));
    return 1;
}
size_t mpn_page(const MPNSession *session) { return session->page; }
size_t mpn_sentence_count(const MPNSession *session) { return session ? session->sentenceCount : 0; }
size_t mpn_literal_tail(const MPNSession *session) { return session ? session->literalTail : 0; }
const char *mpn_exact_pinyin(MPNSession *session, const char *reading) {
    if (!session) return "";
    session->exactPinyin.clear();
    if (!session->lexicon->pinyin || !reading || std::string_view(reading).size() > max_input)
        return session->exactPinyin.c_str();
    try {
        auto words = decode(*session->lexicon, reading, session->hans, true);
        if (!words.empty()) session->exactPinyin = std::move(words.front());
    } catch (...) {}
    return session->exactPinyin.c_str();
}
