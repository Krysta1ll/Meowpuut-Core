"""Compile the bundled mimo-tc, mimo-sc, and mimo-jp lexicons into portable UTF-8 tables."""
import collections
import functools
import pathlib
import re
import sys
import json
import sqlite3
import hashlib
import math
import unicodedata
import subprocess
import statistics
from japanese_readings import pinyin_kana
from mimo_binary import write_mimo_binaries
from mimo_jp_compile import write_mimo_jp
from mimo_latin import latin_rows
from mimo_pairs import counted_words, default_text, spoken_counts, write_pair_table
from mimo_phrases import phrases
from mimo_names import names
from mimo_pipeline import clamp_i16_weight, load_mimo_sc_dataset, load_mimo_tc_dataset, numbered_pinyin

ROOT = pathlib.Path(__file__).resolve().parent.parent
KEYS = dict(zip("bpmfdtnlgkhjqxZCSrzcsiuvaoeEAIOUMNKGR12345",
                "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/- 6347"))
FINALS = {
    "a": "a", "o": "o", "e": "e", "eh": "E", "ai": "A", "ei": "I",
    "ao": "O", "ou": "U", "an": "M", "en": "N", "ang": "K", "eng": "G",
    "er": "R", "i": "i", "ia": "ia", "io": "io", "ie": "iE",
    "iai": "iA", "iao": "iO", "iu": "iU", "iou": "iU",
    "ian": "iM", "in": "iN", "iang": "iK", "ing": "iG",
    "u": "u", "ua": "ua", "uo": "uo", "uai": "uA", "ui": "uI", "uei": "uI",
    "uan": "uM", "un": "uN", "uen": "uN", "uang": "uK", "ueng": "uG", "ong": "uG",
    "v": "v", "ve": "vE", "van": "vM", "vn": "vN", "iong": "vG",
}

@functools.lru_cache(maxsize=None)
def encode(syllable):
    match = re.fullmatch(r"([a-züê]+)([1-5])", syllable)
    if not match:
        return None
    sound, tone = match.groups()
    sound = sound.replace("ü", "v").replace("ê", "eh")
    if sound.startswith("y"):
        tail = sound[1:]
        sound = tail if tail.startswith("i") else "v" + tail[1:] if tail.startswith("u") else "i" + tail
    elif sound.startswith("w"):
        tail = sound[1:]
        sound = tail if tail.startswith("u") else "u" + tail
    if sound == "r":
        sound = "er"
    if sound == "m":
        sound = "mu"
    initial, final = "", sound
    match = re.match(r"^(zh|ch|sh|[bpmfdtnlgkhjqxrzcs])(.+)$", sound)
    if match and sound != "er":
        initial, final = match.groups()
    if initial in {"j", "q", "x"} and final.startswith("u"):
        final = "v" + final[1:]
    if final == "i" and initial in {"zh", "ch", "sh", "r", "z", "c", "s"}:
        final = ""
    phonetic = FINALS.get(final, "" if not final else None)
    if phonetic is None:
        return None
    initial = {"zh": "Z", "ch": "C", "sh": "S"}.get(initial, initial)
    return "".join(KEYS[symbol] for symbol in initial + phonetic + tone)


def authored(name):
    for number, line in enumerate((ROOT / "Resources/Lexicon" / name).read_text().splitlines(), 1):
        if not line or line.startswith("#"): continue
        fields = line.split("|")
        yield number, fields


def imported_words(converter=None):
    tc_data = load_mimo_tc_dataset()
    entries = {(hant, hans, code): int(weight) for hant, hans, code, weight in tc_data["entries"]}
    skipped = tc_data.get("skipped_rows", 0)
    print(f"mimo-tc source: {len(entries):,} accepted readings; {skipped} unsupported/mismatched rows excluded")
    return entries, tc_data.get("st_map", {})


NAME_BASE, NAME_STEP = 2400, 450  # weight of a name: base + step x ln(Wikipedia editions that have it)
TERM_WEIGHT = 3200               # weight of a working word of Resources/Lexicon/terms.tsv


def merge_simplified(tc_bundle, converter, with_phrases):
    words, st_map = tc_bundle
    sc_data = load_mimo_sc_dataset()
    canonical = {}
    for (hant, hans, reading), weight in words.items():
        if (hans, reading) not in canonical or canonical[(hans, reading)][1] < weight:
            canonical[(hans, reading)] = (hant, weight)
    merged = {(hant, hans, reading): weight for (hans, reading), (hant, weight) in canonical.items()}
    native_keys = set(merged)
    valid = [(hans, reading, float(freq)) for hans, reading, freq in sc_data["entries"]]
    skipped = int(sc_data.get("excluded_invalid_rows", 0))
    missing = sorted({hans for hans, reading, _ in valid if (hans, reading) not in canonical})
    converted = subprocess.run([converter, "--traditional"], input="\n".join(missing) + "\n", text=True, capture_output=True, check=True).stdout.splitlines()
    if len(converted) != len(missing): raise ValueError("Traditional conversion mismatch")
    traditional = {h: st_map.get(h, t) for h, t in zip(missing, converted)}
    def source_weight(frequency):
        return clamp_i16_weight(700 * math.log1p(max(0, frequency) * 1e12))
    differences = [source_weight(frequency) - canonical[(hans, reading)][1]
        for hans, reading, frequency in valid
        if (hans, reading) in canonical and frequency > 0 and canonical[(hans, reading)][1] > 0]
    offset = round(statistics.median(differences)) if differences else 0
    merged = {key: clamp_i16_weight(weight + offset - 350) for key, weight in merged.items()}
    duplicates = added = 0
    for hans, reading, frequency in valid:
        existing = canonical.get((hans, reading))
        hant = existing[0] if existing else traditional[hans]
        key = (hant, hans, reading)
        merged[key] = source_weight(frequency)
        if existing or frequency >= 1e-11: native_keys.add(key)
        duplicates += existing is not None
        added += existing is None
    # Phrases (scripts/mimo_phrases.py): written and read as their words are; main() weights them by count.
    best = {}
    for key, weight in merged.items():
        if key[1] not in best or merged[best[key[1]]] < weight: best[key[1]] = key
    phrase_counts = {}
    for parts, written, spoken in phrases() if with_phrases else []:
        found = [best.get(part) for part in parts]
        if None in found or "".join(parts) in best: continue
        key = ("".join(k[0] for k in found), "".join(parts), " ".join(k[2] for k in found))
        merged[key] = 1
        native_keys.add(key)
        phrase_counts[key] = (written, spoken)
    # Names (scripts/mimo_names.py, from Wikidata) and the project's own lists of working words of many
    # fields (Resources/Lexicon/terms.tsv), read character by character. They are typed in full or not
    # at all, so they sit below everyday words; a name sits higher the better known it is.
    name_keys = set()
    def add_fixed(hant, hans, weight):
        found = [best.get(character) for character in hans]
        if hans in best or None in found or len(hant) != len(hans): return
        key = (hant, hans, " ".join(k[2] for k in found))
        merged[key] = max(merged.get(key, 0), weight)
        native_keys.add(key)
        name_keys.add(key)
    for hant, hans, editions in names():
        add_fixed(hant, hans, clamp_i16_weight(NAME_BASE + NAME_STEP * math.log(editions)))
    for line in (ROOT / "Resources/Lexicon/terms.tsv").read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            hant, hans = line.split("\t")[:2]
            add_fixed(hant, hans, TERM_WEIGHT)
    report = {"name": "mimo", "version": "1.0", "license": "see LICENSE and THIRD_PARTY.md",
              "lexicons": ["mimo-tc", "mimo-sc", "mimo-jp"],
              "simplified_rows": len(valid), "duplicate_readings_merged": duplicates,
              "new_readings": added, "excluded_invalid_rows": skipped,
              "phrases": len(phrase_counts), "names": len(name_keys), "opencc_st_phrases_loaded": len(st_map),
              "merged_readings": len(merged), "native_hot_readings": len(native_keys),
              "traditional_only_scale_offset": offset, "traditional_only_preference_discount": 350,
              "note": "All valid new readings remain in indexed SQLite; frequent words also enter native segmentation."}
    print("mimo-sc merge:", json.dumps(report, ensure_ascii=False))
    return merged, native_keys, report, phrase_counts, name_keys


# Zhuyin is typed in Taiwan, so its index takes McBopomofo (mimo-tc) evidence over the
# mainland frequency that `merge_simplified` puts on shared words.
ZHUYIN_UNLISTED_READING_PENALTY = 1400  # reading mimo-tc does not list for a word it has
ZHUYIN_EXTRA_FORM_MIN_WEIGHT = 400      # mimo-tc weight needed to keep a second Traditional form


def zhuyin_weights(words, reviewed, offset):
    """Weight of each (hant, hans, reading) in the zhuyin index.

    Multi-character words in mimo-tc: at least the mimo-tc weight + offset (no 350 discount).
    A reading mimo-tc does not list for a word it has (often a mainland reading): lowered.
    Single characters keep their merged weight: mimo-tc gives all readings of a character one weight.
    Reviewed overlay rows keep their own weight."""
    tc_raw = {(hant, hans, code): int(weight) for hant, hans, code, weight in load_mimo_tc_dataset()["entries"]}
    tc_weight, tc_readings = {}, collections.defaultdict(set)
    for (hant, hans, code), weight in tc_raw.items():
        tc_weight[(hant, hans, code)] = clamp_i16_weight(weight + offset)
        tc_readings[hant].add(code)
    result = {}
    for key, weight in words.items():
        hant, _, code = key
        if key not in reviewed:
            if len(hant) > 1 and key in tc_weight:
                weight = max(weight, tc_weight[key])
            elif hant in tc_readings and code not in tc_readings[hant]:
                weight = max(1, weight - ZHUYIN_UNLISTED_READING_PENALTY)
        result[key] = weight
    return result, tc_raw


def abbreviations(sounds):
    """Initials keys of a word, each syllable as its first letter and its tone (知道了: z4d4l5),
    and, when a syllable starts with zh, ch or sh, the same with those typed in full (zh4d4l5)."""
    short = "".join(s[0] + s[-1] for s in sounds)
    full = "".join((s[:2] if s[:2] in ("zh", "ch", "sh") else s[0]) + s[-1] for s in sounds)
    return {short, full}


def toneless(code):
    return re.sub("[1-5]", "", code)


COUNT_SPOKEN = 30         # a spoken occurrence counts as this many written ones in the word counts (dev: 10 and 100 no better)
ZHUYIN_COUNT_SHARE = 0.5  # how far a zhuyin weight moves towards the count (dev: 0.25 and 0.75 worse)


def count_scale(words, skip, phrase_counts, report):
    """A count of the pair sources on the weight scale: 700 ln(1 + count) plus the median difference
    to the merged weight, taken separately for words and single characters. The count of a word is
    that of its text (DataSources/mimo-pairs, U rows), the count of a phrase its own.
    A text the sources never have counts as zero: in this much text that says it is rare.
    Returns key -> weight."""
    counted = {text: written + COUNT_SPOKEN * spoken for text, (written, spoken) in counted_words().items()}
    top = {}
    for key, weight in words.items():
        if key not in skip: top[key[1]] = max(top.get(key[1], 0), weight)
    differences = {True: [], False: []}
    for text, weight in top.items():
        if (n := counted.get(text, 0)) > 0: differences[len(text) > 1].append(weight - 700 * math.log1p(n))
    offsets = {multi: round(statistics.median(values)) if values else 0 for multi, values in differences.items()}
    report["count_word_offset"], report["count_character_offset"] = offsets[True], offsets[False]
    def count_weight(key):
        n = counted.get(key[1], 0)
        if key in phrase_counts: n = phrase_counts[key][0] + COUNT_SPOKEN * phrase_counts[key][1]
        return clamp_i16_weight(700 * math.log1p(n) + offsets[len(key[1]) > 1])
    return count_weight


def pinyin_weights(words, native, reviewed, fixed, tc_raw, offset, count_weight, report):
    """Weight of each (hant, hans, reading) in the pinyin index: a blend of the sources.

    Multi-character words: mean of the merged weight, mimo-tc weight + offset (words both
    mimo-sc and mimo-tc have) and the count of the word in the pair sources on the same scale
    (see count_scale). Single characters: mean of the merged weight and the count on the same scale.
    The count is of the text, whatever its reading: the most likely reading of a text is blended
    and its other readings move by the same amount, so their order stays.
    Taiwan and Hong Kong words from OpenCC (regional_tw_hk): at least mimo-tc weight + offset - 350.
    Reviewed overlay rows and the names (`fixed`) keep their weight."""
    sc_keys = {(hans, code) for hans, code, _ in load_mimo_sc_dataset()["entries"]}
    regional = load_mimo_tc_dataset().get("regional_tw_hk", {})
    def listed(key):
        """Mean of the word lists' weights for a reading, and how many there are."""
        tc_weight = clamp_i16_weight(tc_raw[key] + offset) if key in tc_raw else None
        values = [words[key]]
        if len(key[1]) > 1 and tc_weight is not None and (key[1], key[2]) in sc_keys: values.append(tc_weight)
        return sum(values) / len(values), len(values), tc_weight
    top = {}
    for key, weight in words.items():
        if key not in reviewed and key not in fixed and (key[1] not in top or words[top[key[1]]] < weight): top[key[1]] = key
    shift = {}
    for text, key in top.items():
        counted = count_weight(key)
        if counted is not None:
            mean, n, _ = listed(key)
            shift[text] = (mean * n + counted) / (n + 1) - mean
    result = {}
    for key, weight in words.items():
        hant, hans, code = key
        if key not in reviewed:
            mean, _, tc_weight = listed(key)
            weight = clamp_i16_weight(mean + shift.get(hans, 0))
            if len(hans) > 1 and hant in regional and tc_weight is not None: weight = max(weight, clamp_i16_weight(tc_weight - 350))
        result[key] = weight
    # Reviewed rows were weighted against the merged weights, so each stays below the indexed words
    # with the same letters that it was below before the blend.
    above = collections.defaultdict(list)
    for key, weight in words.items():
        if key in native and key not in reviewed: above[toneless(key[2]).replace(" ", "")].append((weight, result[key]))
    lowered = 0
    for key in reviewed:
        if key not in words: continue
        limit = min((new - 1 for old, new in above[toneless(key[2]).replace(" ", "")] if old > words[key]), default=words[key])
        if limit < result[key]:
            result[key] = max(1, limit); lowered += 1
    report["reviewed_rows_lowered_for_pinyin"] = lowered
    return result


def counted_shift(weights, fixed, count_weight, share):
    """Moves every reading of a text towards the count of the text by `share` of the gap between
    the count and the weight of its most likely reading. Reviewed rows and names (`fixed`) are not
    moved, but each stays below the words with the same reading that it was below before."""
    top = {}
    for key, weight in weights.items():
        if key not in fixed and (key[1] not in top or weights[top[key[1]]] < weight): top[key[1]] = key
    shift = {text: share * (count_weight(key) - weights[key]) for text, key in top.items()}
    result = {key: clamp_i16_weight(weight + shift[key[1]]) if key[1] in shift and key not in fixed else weight for key, weight in weights.items()}
    letters = lambda key: toneless(key[2]).replace(" ", "")  # a first tone is typed without a mark, so it meets every tone
    above = collections.defaultdict(list)
    for key, weight in weights.items():
        if key not in fixed: above[letters(key)].append((weight, result[key]))
    for key in fixed:
        if key not in weights: continue
        limit = min((new - 1 for old, new in above[letters(key)] if old > weights[key]), default=weights[key])
        if limit < result[key]: result[key] = max(1, limit)
    return result


# Spoken text (Tatoeba, Common Voice, the project's everyday sentences) as evidence for word weights.
SPOKEN_MIN_COUNT = 2   # spoken occurrences needed before a word is raised
SPOKEN_SHARE = 1.5     # a word below its spoken weight is raised by this times the gap (dev: 1.0 and 2.0 worse)


def spoken_weights(weights, reviewed, report, label):
    """Raises words people say more often than written text suggests.

    A word's spoken weight is 700 ln(1 + spoken count) plus the median difference to the
    weights of the words with spoken counts (as count_scale does). A word of two or more
    characters seen SPOKEN_MIN_COUNT+ times below its spoken weight is raised by SPOKEN_SHARE
    times the gap; every reading of the word by the same amount, so their order stays. Its
    initials keys follow, being derived from the full weight. Reviewed rows are left alone. A variant
    word of Resources/Lexicon/pair-defaults.tsv (谢谢妳) is not raised above its default (谢谢你)."""
    spoken = spoken_counts()
    best = {}
    for key, weight in weights.items():
        if len(key[1]) > 1 and key not in reviewed: best[key[1]] = max(best.get(key[1], 0), weight)
    seen = {hans: weight for hans, weight in best.items() if spoken.get(hans, 0) >= SPOKEN_MIN_COUNT}
    offset = statistics.median(weight - 700 * math.log1p(spoken[hans]) for hans, weight in seen.items())
    bonus = {hans: round(SPOKEN_SHARE * (700 * math.log1p(spoken[hans]) + offset - weight)) for hans, weight in seen.items()}
    bonus = {hans: b for hans, b in bonus.items() if b > 0}
    report[f"spoken_offset_{label}"], report[f"spoken_raised_{label}"] = round(offset), len(bonus)
    result = {key: clamp_i16_weight(weight + bonus[key[1]]) if key[1] in bonus and key not in reviewed else weight
              for key, weight in weights.items()}
    raised = {}
    for key, weight in result.items():
        raised[key[1]] = max(raised.get(key[1], 0), weight)
    for key, weight in result.items():
        default = default_text(key[1]) if key[1] in bonus else None
        if default in raised and weight >= raised[default]:
            result[key] = max(weights[key], raised[default] - 1)
    return result


def main(destination, converter, *options):
    output = pathlib.Path(destination)
    output.mkdir(parents=True, exist_ok=True)
    associations = json.loads((ROOT / "Resources/Lexicon/associations.json").read_text())
    (output / "associations.json").write_text(json.dumps(associations, ensure_ascii=False))
    words, native_keys, merge_report, phrase_counts, name_keys = merge_simplified(imported_words(converter), converter, "--no-phrases" not in options)
    (output / "lexicon-merge-report.json").write_text(json.dumps(merge_report, indent=2))
    authored_count = 0
    reviewed_groups = {}
    derived_keys = set()
    for overlay_file in ("chinese.tsv", "community.tsv"):
        for number, fields in authored(overlay_file):
            if len(fields) != 4: raise ValueError(f"{overlay_file}:{number}: expected 4 fields")
            traditional, simplified, code, priority = fields
            sounds = code.split()
            if not all(encode(s) for s in sounds): raise ValueError(f"{overlay_file}:{number}: invalid pinyin")
            if not traditional or not simplified or not priority.isdigit(): raise ValueError(f"{overlay_file}:{number}: invalid word")
            weight = int(priority)
            reviewed_groups[(simplified, code)] = ((traditional, simplified, code), weight)
            words[(traditional, simplified, code)] = weight
            native_keys.add((traditional, simplified, code))
            authored_count += 1
            # Character readings are derived only from our own explicitly supplied words.
            if len(traditional) == len(simplified) == len(sounds):
                for hant, hans, sound in zip(traditional, simplified, sounds):
                    key = (hant, hans, sound)
                    words[key] = max(words.get(key, 0), min(1000, weight // 10))
                    native_keys.add(key)
                    derived_keys.add(key)
    canonical = {}
    native_groups = {(key[1], key[2]) for key in native_keys}
    for key, weight in words.items():
        group = (key[1], key[2])
        if group not in canonical or canonical[group][1] < weight: canonical[group] = (key, weight)
    # Reviewed spellings/weights win even when lower than an upstream variant.
    canonical.update(reviewed_groups)
    merge_report["reviewed_readings"] = len(reviewed_groups)
    merge_report["derived_character_readings"] = len(derived_keys)
    merge_report["final_duplicate_variants_removed"] = len(words) - len(canonical)
    words = {key: weight for key, weight in canonical.values()}
    native_keys = {key for key in words if (key[1], key[2]) in native_groups}
    merge_report["final_readings"] = len(words)
    merge_report["final_native_readings"] = len(native_keys)
    rows = {"zhuyin": {}, "pinyin": {}}
    def add(mode, bare, reading, hant, hans, weight):
        key = (bare, reading, hant, hans)
        rows[mode][key] = max(rows[mode].get(key, 0), weight)
    context = {}
    reviewed_keys = {key for key, _ in reviewed_groups.values()}
    # --no-counts: weights from the word lists alone. The pair and phrase counts are made with such a
    # lexicon, so that how the text is split into words does not depend on earlier counts.
    counts = "--no-counts" not in options
    count_weight = count_scale(words, reviewed_keys | set(phrase_counts), phrase_counts, merge_report) if counts else lambda key: None
    for key in phrase_counts:
        if key in words and key not in reviewed_keys: words[key] = count_weight(key)
    zhuyin_weight, tc_raw = zhuyin_weights(words, reviewed_keys, merge_report["traditional_only_scale_offset"])
    pinyin_weight = pinyin_weights(words, native_keys, reviewed_keys, name_keys, tc_raw, merge_report["traditional_only_scale_offset"], count_weight, merge_report)
    if counts:
        zhuyin_weight = counted_shift(zhuyin_weight, reviewed_keys | name_keys, count_weight, ZHUYIN_COUNT_SHARE)
        zhuyin_weight = spoken_weights(zhuyin_weight, reviewed_keys, merge_report, "zhuyin")
        pinyin_weight = spoken_weights(pinyin_weight, reviewed_keys, merge_report, "pinyin")
    for (hant, hans, code), merged_weight in words.items():
        if (hant, hans, code) not in native_keys: continue
        sounds = code.split()
        phonetic = "".join(encode(sound) for sound in sounds)
        add("zhuyin", re.sub("[ 6347]", "", phonetic), phonetic, hant, hans, zhuyin_weight[(hant, hans, code)])
        weight = pinyin_weight[(hant, hans, code)]
        aliases = {"".join(sounds)}
        aliases |= {re.sub(r"([nl])ve", r"\1ue", p) for p in list(aliases)}
        # Full pinyin is primary. Initial abbreviations remain available with a lower prior.
        if len(sounds) > 1:
            for short in abbreviations(sounds):
                add("pinyin", re.sub("[1-5]", "", short), short, hant, hans, max(1, weight // 8))
        for reading in aliases: add("pinyin", re.sub("[1-5]", "", reading), reading, hant, hans, weight)
        if len(hant) > 1: context[hant] = max(context.get(hant, 0), merged_weight)
    # The merge keeps one Traditional form per (Simplified, reading). Zhuyin output is Traditional
    # only, so it also gets the other mimo-tc forms (讚 next to 贊, 準 next to 准). A word gets its
    # mimo-tc weight + offset; a single character is placed below the kept form by its mimo-tc
    # difference, because mimo-tc gives all readings of a character one weight.
    offset = merge_report["traditional_only_scale_offset"]
    kept = {(hans, code): (hant, zhuyin_weight[(hant, hans, code)]) for (hant, hans, code) in words if (hant, hans, code) in native_keys}
    zhuyin_have = {(hant, hans, reading) for (_, reading, hant, hans) in rows["zhuyin"]}
    extra_forms = 0
    for (hant, hans, code), raw in tc_raw.items():
        if len(hant) > 4 or raw < ZHUYIN_EXTRA_FORM_MIN_WEIGHT: continue
        phonetic = "".join(encode(sound) for sound in code.split())
        if (hant, hans, phonetic) in zhuyin_have: continue
        weight = clamp_i16_weight(raw + offset)
        if len(hant) == 1 and (hans, code) in kept:
            kept_hant, kept_weight = kept[(hans, code)]
            weight = clamp_i16_weight(kept_weight - max(0, tc_raw.get((kept_hant, hans, code), raw) - raw))
        add("zhuyin", re.sub("[ 6347]", "", phonetic), phonetic, hant, hans, weight)
        extra_forms += 1
    merge_report["zhuyin_extra_traditional_forms"] = extra_forms
    # Context-only observations steer adjacent word segments and next-word predictions.
    for number, fields in authored("context.tsv"):
        if len(fields) != 2: raise ValueError(f"context.tsv:{number}: expected 2 fields")
        phrase, priority = fields
        if not 2 <= len(phrase) <= 5 or not all('\u3400' <= c <= '\u9fff' for c in phrase) or not priority.isdigit() or not 1 <= int(priority) <= 10000:
            raise ValueError(f"context.tsv:{number}: invalid phrase or priority")
        context[phrase] = max(context.get(phrase, 0), int(priority))
    sorted_context = sorted(context.items())
    # English words inside pinyin (scripts/mimo_latin.py): upper-case keys, so the Japanese side never sees them.
    rows_without_latin = sorted(rows["pinyin"].items(), key=lambda row: (row[0][0], -row[1], row[0][1:]))
    for key, weight in latin_rows(): rows["pinyin"][key] = weight
    merge_report["latin_words"] = len(latin_rows())
    sorted_rows = {}
    for mode, entries in rows.items():
        ordered = sorted(entries.items(), key=lambda row: (row[0][0], -row[1], row[0][1:]))
        sorted_rows[mode] = ordered
        with (output / f"native-{mode}.tsv").open("w") as dest:
            dest.write(f"MEOWPUUT-{mode.upper()}-1\n")
            for (bare, reading, hant, hans), weight in ordered:
                dest.write(f"{bare}\t{reading}\t{hant}\t{hans}\t{weight}\n")
            dest.write("# context\n")
            for word, weight in sorted_context: dest.write(f"{word}\t{weight}\n")
    bin_stats = write_mimo_binaries(output, sorted_rows["pinyin"], sorted_rows["zhuyin"], sorted_context)
    merge_report.update(bin_stats)
    if counts: merge_report.update(write_pair_table(output, sorted_rows["pinyin"], sorted_rows["zhuyin"]))
    japanese = {}
    def add_japanese(reading, word, weight):
        if not re.fullmatch("[a-z'-]+", reading) or not word or not 0 < weight <= 10000:
            raise ValueError(f"invalid Japanese entry: {reading!r}, {word!r}, {weight}")
        entries = japanese.setdefault(reading, {})
        entries[word] = max(entries.get(word, 0), weight)
    for number, fields in authored("japanese.tsv"):
        if len(fields) != 3: raise ValueError(f"japanese.tsv:{number}: expected 3 fields")
        readings, word, weight = fields
        for reading in readings.split(","):
            add_japanese(reading, word, int(weight))
    mimo_jp = ROOT / "DataSources/mimo-jp"
    manifest = json.loads((mimo_jp / "SOURCE.json").read_text())
    # The generated snapshot and the notice files must match the source record.
    for name, checksum in manifest["files"].items():
        if hashlib.sha256((mimo_jp / name).read_bytes()).hexdigest() != checksum:
            raise ValueError(f"mimo-jp data changed: {name}; rebuild it with scripts/mimo_jp_pipeline.py")
    # The 60 reviewed rows of mimo-jp 1.x; pinyin and weights are written by the project.
    daily = (ROOT / "Resources/Lexicon/japanese-reviewed.tsv").read_bytes()
    for number, line in enumerate(daily.decode("utf-8").splitlines(), 1):
        if not line or line.startswith("#"): continue
        fields = line.split("|")
        if len(fields) != 5: raise ValueError(f"japanese-reviewed.tsv:{number}: expected 5 fields")
        kana, word, romaji, chinese, priority = fields
        if not re.fullmatch(r"[ぁ-ゖー]+", kana) or not priority.isdigit():
            raise ValueError(f"japanese-reviewed.tsv:{number}: invalid reading or priority")
        weight = int(priority)
        if not 4000 < weight <= 10000:
            raise ValueError(f"japanese-reviewed.tsv:{number}: priority must leave room for a lower-weight alias")
        add_japanese(romaji, word, weight)
        alias = pinyin_kana(word, chinese.split())
        if alias != romaji:
            contains_kana = any("\u3040" <= char <= "\u30ff" for char in word)
            alias_weight = max(4000, weight - 3000) if contains_kana else 3500
            add_japanese(alias, word, alias_weight)
    japanese = {reading: [{"text": word, "weight": weight} for word, weight in entries.items()]
                for reading, entries in japanese.items()}
    for values in japanese.values(): values.sort(key=lambda value: (-value["weight"], value["text"]))
    (output / "native-japanese.json").write_text(json.dumps(japanese, ensure_ascii=False, sort_keys=True))
    with (output / "native-japanese.tsv").open("w") as dest:
        for reading in sorted(japanese):
            for entry in japanese[reading]:
                dest.write(f"{reading}|{entry['text']}|{entry['weight']}\n")
    jp_stats = write_mimo_jp(output, rows_without_latin, japanese, ROOT / ".build/mimo-jp-review")
    base_sha256 = hashlib.sha256((output / "mimo_base.bin").read_bytes()).hexdigest()
    merge_report.update(jp_stats)
    merge_report["mimo_base_sha256"] = base_sha256
    merge_report["japanese_readings_legacy"] = len(japanese)
    (output / "lexicon-merge-report.json").write_text(json.dumps(merge_report, indent=2))
    (output / "mimo-manifest.json").write_text(json.dumps({
        "name": "mimo", "version": "1.0", "license": "see LICENSE and THIRD_PARTY.md",
        "readings": len(words), "lexicons": ["mimo-tc", "mimo-sc", "mimo-jp"],
        "binary_format": "MIMO-BASE-1 / MIMO-BIN-1 (i16 Log-Probability SoA); MIMO-JP-1",
        "binary_files": ["mimo_base.bin", "mimo_jp.bin", "mimo_pairs.bin"],
        "mimo_base_sha256": base_sha256,
        "japanese_words": jp_stats["japanese_words"], "japanese_keys": jp_stats["japanese_keys"],
        "key": "simplified spelling + tone-qualified reading",
        "frequency_preference": "mimo-sc + mimo-tc + counted text, calibrated; OpenCC STPhrases; reviewed overlays last",
        "sources": ["mimo-tc (MIT)", "mimo-sc (MIT)", "OpenCC (Apache-2.0)", "jieba (MIT)"],
        "japanese_sources": ["Mozc dictionary_oss (BSD-3-Clause AND NAIST-2003)", "UniDic unidic-cwj 2025.12 (BSD-3-Clause)",
                             "OpenCC JPShinjitaiCharacters (Apache-2.0)", "Unihan 18.0.0 (Unicode-3.0)",
                             "mimo-sc and mimo-tc readings (MIT)", "Meowpuut authored"]
    }, ensure_ascii=False, indent=2))
    database = output / "language-evidence.sqlite"
    if database.exists(): database.unlink()
    db = sqlite3.connect(database)
    db.execute("CREATE TABLE words(language TEXT, reading TEXT, word TEXT, weight INTEGER, PRIMARY KEY(language,reading,word)) WITHOUT ROWID")
    db.execute("CREATE TABLE lexicon(reading TEXT, phonetic TEXT, bare TEXT, hant TEXT, hans TEXT, weight INTEGER, toned TEXT, PRIMARY KEY(reading,phonetic,hant,hans)) WITHOUT ROWID")
    def record(language, reading, word, weight):
        db.execute("INSERT INTO words VALUES(?,?,?,?) ON CONFLICT DO UPDATE SET weight=max(weight,excluded.weight)", (language, reading, word, weight))
    for (hant, hans, code), weight in words.items():
        reading = re.sub("[1-5 ]", "", code)
        phonetic = "".join(encode(sound) for sound in code.split())
        weight = zhuyin_weight[(hant, hans, code)]
        db.execute("INSERT OR REPLACE INTO lexicon VALUES(?,?,?,?,?,?,?)", (reading, phonetic, re.sub("[ 6347]", "", phonetic), hant, hans, weight, code.replace(" ", "")))
    for reading, items in japanese.items():
        for item in items: record("ja", reading.replace("'", ""), item["text"], item["weight"])
    db.execute("CREATE INDEX lexicon_phonetic ON lexicon(phonetic)")
    db.execute("CREATE INDEX lexicon_bare ON lexicon(bare)")
    db.commit(); db.close()
    print(f"Meowpuut overlay: {authored_count} Chinese rows; {len(japanese)} Japanese readings; total {len(words):,} Chinese readings")


if __name__ == "__main__":
    main(*sys.argv[1:])
