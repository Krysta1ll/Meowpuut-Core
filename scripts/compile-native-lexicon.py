"""Compile the bundled mimo-tc, mimo-sc, and mimo-jp lexicons into portable UTF-8 tables."""
import collections
import functools
import itertools
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
from blocked_terms import drop_hit, en_hit, ja_hit, sections as blocked_sections, zh_hit, zh_terms, zh_whole_only
from mimo_binary import _is_fully_spelled_reading, write_mimo_binaries
from mimo_jp_compile import write_mimo_jp
from mimo_latin import latin_rows
from mimo_pairs import counted_words, default_text, split_counts, spoken_counts, write_pair_table
from mimo_phrases import phrases
from mimo_names import names
from mimo_pipeline import clamp_i16_weight, load_mimo_sc_dataset, load_mimo_tc_dataset, numbered_pinyin

ROOT = pathlib.Path(__file__).resolve().parent.parent


def write_zhuyin_initials(db):
    """Materialize the same first 32 rows as the former phonetic GLOB query.
    Never-volunteer words (scripts/blocked_terms.py) get no initials keys."""
    initials = "1qaz2wsxedcrfv5tgbyhn"
    tones = " 6347"
    db.execute("""CREATE TABLE zhuyin_initials(
        initials TEXT, position INTEGER, hant TEXT, hans_variant TEXT, weight INTEGER,
        hans TEXT GENERATED ALWAYS AS (coalesce(hans_variant,hant)) VIRTUAL,
        PRIMARY KEY(initials,position)) WITHOUT ROWID""")

    def index_rows():
        positions = collections.defaultdict(int)
        # The previous phonetic index breaks weight ties in phonetic and
        # primary-key order. Keep that order even when the new table is used.
        rows = db.execute("""SELECT phonetic,hant,hans,weight FROM lexicon
            WHERE length(hant) BETWEEN 2 AND 4
            ORDER BY weight DESC,phonetic,reading,hant,hans""")
        for phonetic, hant, hans, weight in rows:
            if not phonetic or phonetic[0] not in initials or phonetic[-1] not in tones or zh_hit(hans):
                continue
            # A GLOB * can cross extra syllables. The first initial must be
            # first, and each later initial must immediately follow a tone.
            following = [key for tone, key in zip(phonetic, phonetic[1:])
                         if tone in tones and key in initials]
            for suffix in set(itertools.combinations(following, len(hant) - 1)):
                keys = phonetic[0] + "".join(suffix)
                position = positions[keys]
                if position < 32:
                    yield keys, position, hant, None if hans == hant else hans, weight
                    positions[keys] += 1

    db.executemany("INSERT INTO zhuyin_initials(initials,position,hant,hans_variant,weight) VALUES(?,?,?,?,?)", index_rows())


def write_evidence_lexicon(db, words, zhuyin_weight):
    # Keep the import's duplicate resolution and tie order. Secondary indexes
    # in the shipped table then need only a small rowid, not the whole key.
    db.execute("""CREATE TEMP TABLE lexicon_import(
        reading TEXT, phonetic TEXT, hant TEXT, hans TEXT, weight INTEGER, toned TEXT,
        PRIMARY KEY(reading,phonetic,hant,hans)) WITHOUT ROWID""")

    def evidence_rows():
        for hant, hans, code in words:
            reading = re.sub("[1-5 ]", "", code)
            phonetic = "".join(encode(sound) for sound in code.split())
            yield reading, phonetic, hant, hans, zhuyin_weight[(hant, hans, code)], code.replace(" ", "")

    db.executemany("INSERT OR REPLACE INTO lexicon_import VALUES(?,?,?,?,?,?)", evidence_rows())
    # The untoned keys already live in their indexes. Derive them from the
    # toned readings so the table does not store a second copy of each key.
    db.execute("""CREATE TABLE lexicon(
        reading TEXT GENERATED ALWAYS AS
            (replace(replace(replace(replace(replace(toned,'1',''),'2',''),'3',''),'4',''),'5','')) VIRTUAL,
        phonetic TEXT,
        bare TEXT GENERATED ALWAYS AS
            (replace(replace(replace(replace(replace(phonetic,' ',''),'6',''),'3',''),'4',''),'7','')) VIRTUAL,
        hant TEXT, hans_variant TEXT,
        hans TEXT GENERATED ALWAYS AS (coalesce(hans_variant,hant)) VIRTUAL,
        weight INTEGER, toned TEXT)""")
    db.execute("""INSERT INTO lexicon(phonetic,hant,hans_variant,weight,toned)
        SELECT phonetic,hant,nullif(hans,hant),weight,toned FROM lexicon_import
        ORDER BY reading,phonetic,hant,hans""")
    db.execute("DROP TABLE lexicon_import")
    for column in ("reading", "phonetic", "bare"):
        db.execute(f"CREATE INDEX lexicon_{column} ON lexicon({column}, weight DESC)")


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


@functools.lru_cache(maxsize=None)
def tc_dataset():
    """mimo-tc with three kinds of rows corrected, each found from the data itself:

    - A Traditional character whose Simplified form depends on the reading: the platform converter
      (mimo_pipeline) writes the character alone one way (藉 -> 借), but the words that read it so
      keep it (狼藉 ji2). A character row takes the form most of those words have (藉 ji2, 著 zhu4).
    - A word whose converted Simplified form mimo-sc does not have, where mimo-sc has the word with
      the same reading written with the Traditional character kept (俱樂部: 具乐部 -> 俱乐部, 乾隆:
      干隆 -> 乾隆). Taken when that is the only such word, it is not one of mimo-sc's rare rows
      (below 1e-11), and each kept character is written so with that reading in mimo-sc words of
      1e-10 or more.
    - A Simplified character that McBopomofo also lists as an old character of its own, with old
      readings never seen in its text (还 bu4, 过 guo1): such a row (weight 1) is dropped when the
      reading is neither one of the Traditional characters that simplify to it nor in mimo-sc."""
    data = dict(load_mimo_tc_dataset())
    entries = [tuple(row) for row in data["entries"]]
    forms = collections.defaultdict(collections.Counter)
    via = collections.defaultdict(set)
    for hant, hans, code, _ in entries:
        if len(hant) > 1 and len(hant) == len(hans):
            for t, s, sound in zip(hant, hans, code.split()): forms[(t, sound)][s] += 1
        elif len(hant) == 1 and hant != hans:
            via[hans].add(code)
    sc_entries = load_mimo_sc_dataset()["entries"]
    sc_readings = collections.defaultdict(set)
    sc_words, sc_spelled = collections.defaultdict(list), set()
    sc_texts = {hans for hans, _, _ in sc_entries}
    for hans, code, frequency in sc_entries:
        if len(hans) == 1: sc_readings[hans].add(code)
        else:
            if frequency >= 1e-11: sc_words[code].append(hans)
            if frequency >= 1e-10: sc_spelled.update(zip(hans, code.split()))
    cleaned, remapped, dropped, remapped_words = [], 0, 0, {}
    for hant, hans, code, weight in entries:
        if len(hant) == 1 and hant != hans and forms[(hant, code)][hant] > forms[(hant, code)][hans]:
            hans = hant; remapped += 1
        elif 1 < len(hant) == len(hans) and hant != hans and hans not in sc_words[code]:
            kept = [word for word in sc_words[code] if len(word) == len(hans) and word != hans
                    and all(c in (t, s) for c, t, s in zip(word, hant, hans))]
            if len(kept) == 1 and all(c == s or (c, sound) in sc_spelled for c, s, sound in zip(kept[0], hans, code.split())):
                if hans not in sc_texts: remapped_words[hans] = kept[0]
                hans = kept[0]; remapped += 1
        elif len(hant) == 1 and hant == hans and weight <= 1 and hans in via and code not in via[hans] | sc_readings[hans]:
            dropped += 1; continue
        cleaned.append([hant, hans, code, weight])
    data["entries"], data["remapped_character_forms"], data["dropped_variant_readings"] = cleaned, remapped, dropped
    # Converter spellings of remapped words that mimo-sc never writes (大坂, not 分子): their counts
    # in the source text are of the remapped word.
    data["remapped_words"] = remapped_words
    return data


def imported_words(converter=None):
    tc_data = tc_dataset()
    entries = {(hant, hans, code): int(weight) for hant, hans, code, weight in tc_data["entries"]}
    skipped = tc_data.get("skipped_rows", 0)
    print(f"mimo-tc source: {len(entries):,} accepted readings; {skipped} unsupported/mismatched rows excluded; "
          f"{tc_data['remapped_character_forms']} character forms set by reading; {tc_data['dropped_variant_readings']} old variant readings dropped")
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
    # mimo-sc also lists some Traditional spellings (鹹 next to 咸, 臺北 next to 台北). A row whose text the
    # platform simplifies to another text that is listed with the same reading is that word again:
    # it counts for that text, with the larger of the two frequencies, and its own spelling is kept
    # as a Traditional form of that text (main() adds it below the kept form). Only when each
    # character the platform changes is outside GB2312, so it is no Simplified character of its own
    # (摔跤 stays: 跤 is Simplified too, though the platform writes it 交; 份子 stays the same way).
    def simplified_character(character):
        try: character.encode("gb2312")
        except UnicodeEncodeError: return False
        return True
    listed = {(hans, reading) for hans, reading, _ in valid}
    unknown = sorted({hans for hans, reading, _ in valid if (hans, reading) not in canonical})
    simplified = subprocess.run([converter], input="\n".join(unknown) + "\n", text=True, capture_output=True, check=True).stdout.splitlines()
    if len(simplified) != len(unknown): raise ValueError("Simplified conversion mismatch")
    simplified = dict(zip(unknown, simplified))
    folded, traditional_forms, folded_texts = {}, {}, {}
    for hans, reading, frequency in valid:
        other = simplified.get(hans, hans)
        if (other != hans and len(other) == len(hans) and (other, reading) in listed
                and all(a == b or not simplified_character(a) for a, b in zip(hans, other))):
            if frequency >= 1e-11: traditional_forms[(hans, other, reading)] = frequency
            folded_texts[hans] = other
            hans = other
        folded[(hans, reading)] = max(folded.get((hans, reading), 0.0), frequency)
    traditional_spellings = len(valid) - len(folded)
    valid = [(hans, reading, frequency) for (hans, reading), frequency in folded.items()]
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
    # How far each kept Traditional spelling goes below its Simplified text.
    traditional_forms = {key: max(0, source_weight(folded[(key[1], key[2])]) - source_weight(frequency))
                         for key, frequency in traditional_forms.items()}
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
    name_keys, term_keys = set(), set()
    def add_fixed(hant, hans, weight, is_term=False):
        if len(hant) != len(hans):
            return
        existing = best.get(hans)
        if existing is not None:
            if not is_term or existing in native_keys:
                return
            key = (hant, hans, existing[2]) if is_term and existing[0] != hant else existing
            merged[key] = max(merged.get(existing, 0), weight)
            native_keys.add(key)
            (term_keys if is_term else name_keys).add(key)
            return
        found = [best.get(character) for character in hans]
        if None in found:
            return
        key = (hant, hans, " ".join(k[2] for k in found))
        merged[key] = max(merged.get(key, 0), weight)
        native_keys.add(key)
        (term_keys if is_term else name_keys).add(key)
    for hant, hans, editions in names():
        add_fixed(hant, hans, clamp_i16_weight(NAME_BASE + NAME_STEP * math.log(editions)))
    for line in (ROOT / "Resources/Lexicon/terms.tsv").read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            hant, hans = line.split("\t")[:2]
            add_fixed(hant, hans, TERM_WEIGHT, is_term=True)
    report = {"name": "mimo", "version": "1.0", "license": "see LICENSE and THIRD_PARTY.md",
              "lexicons": ["mimo-tc", "mimo-sc", "mimo-jp"],
              "simplified_rows": len(valid), "traditional_spellings_folded": traditional_spellings, "duplicate_readings_merged": duplicates,
              "new_readings": added, "excluded_invalid_rows": skipped,
              "phrases": len(phrase_counts), "names": len(name_keys), "terms_promoted": len(term_keys), "opencc_st_phrases_loaded": len(st_map),
              "merged_readings": len(merged), "native_hot_readings": len(native_keys),
              "traditional_only_scale_offset": offset, "traditional_only_preference_discount": 350,
              "note": "All valid new readings remain in indexed SQLite; frequent words also enter native segmentation."}
    print("mimo-sc merge:", json.dumps(report, ensure_ascii=False))
    return merged, native_keys, report, phrase_counts, name_keys, term_keys, traditional_forms, folded_texts


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
    tc_raw = {(hant, hans, code): int(weight) for hant, hans, code, weight in tc_dataset()["entries"]}
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


SANDHI_SYLLABLES = {"一": "yi", "不": "bu"}


def sandhi_tones(weights, tc_raw, reviewed, report):
    """mimo-tc lists a word with 一 or 不 both in the character's own tone and in the tone it is said
    with (一杯 yi1 bei1 and yi4 bei1), with one weight. mimo-sc has only the said tone, so the blend
    puts the other one lower. Zhuyin is typed with tones and people type either, so each reading of
    such a word gets the weight of the heaviest. Only when mimo-tc gives them all the same weight."""
    groups = collections.defaultdict(list)
    for key in weights:
        hant, hans, code = key
        sounds = code.split()
        if key in reviewed or len(hans) < 2 or len(sounds) != len(hans): continue
        letters = tuple(toneless(s) if SANDHI_SYLLABLES.get(c) == toneless(s) else s for c, s in zip(hans, sounds))
        if letters != tuple(sounds): groups[(hant, hans, letters)].append(key)
    raised = 0
    for _, keys in sorted(groups.items()):
        if len(keys) < 2 or None in (tc := {tc_raw.get(key) for key in keys}) or len(tc) != 1: continue
        top = max(weights[key] for key in keys)
        for key in keys:
            if weights[key] < top: weights[key] = top; raised += 1
    report["zhuyin_sandhi_readings_raised"] = raised


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


def count_scale(words, skip, phrase_counts, report, folded_texts):
    """A count of the pair sources on the weight scale: 700 ln(1 + count) plus the median difference
    to the merged weight, taken separately for words and single characters. The count of a word is
    that of its text (DataSources/mimo-pairs, U rows), the count of a phrase its own.
    The counting split the text into the most likely words, so a word that is also two words
    (我在 = 我 + 在) was mostly counted as those: the pairs that spell a word (P rows) count for it too.
    A text the sources never have counts as zero: in this much text that says it is rare.
    The sources also hold Traditional text: a Traditional spelling merge_simplified folded into its
    Simplified text (簡 into 简) counts for that text. The source text was simplified with the
    platform converter, so a word tc_dataset spells otherwise (大坂 -> 大阪) takes the count of the
    converter's spelling.
    Returns key -> weight."""
    counted = {
        text: written + COUNT_SPOKEN * spoken
        for text, (written, spoken) in counted_words().items()
        if written > 0 or spoken >= 2
    }
    for text, other in sorted(folded_texts.items()):
        if text in counted: counted[other] = counted.get(other, 0) + counted.pop(text)
    top = {}
    for key, weight in words.items():
        if key not in skip: top[key[1]] = max(top.get(key[1], 0), weight)
    differences = {True: [], False: []}
    for text, weight in top.items():
        if (n := counted.get(text, 0)) > 0: differences[len(text) > 1].append(weight - 700 * math.log1p(n))
    offsets = {multi: round(statistics.median(values)) if values else 0 for multi, values in differences.items()}
    report["count_word_offset"], report["count_character_offset"] = offsets[True], offsets[False]
    # Added after the scale is set, so the words counted whole keep their place on it.
    split = split_counts({key[1] for key in words if len(key[1]) > 1})
    for text, n in split.items(): counted[text] = counted.get(text, 0) + n
    report["count_words_with_split_counts"] = len(split)
    def count_weight(key):
        n = counted.get(key[1], 0)
        if key in phrase_counts: n = phrase_counts[key][0] + COUNT_SPOKEN * phrase_counts[key][1]
        return clamp_i16_weight(700 * math.log1p(n) + offsets[len(key[1]) > 1])
    return count_weight


def pinyin_weights(words, native, reviewed, fixed, tc_raw, offset, count_weight, report, upward_only=frozenset()):
    """Weight of each (hant, hans, reading) in the pinyin index: a blend of the sources.

    Multi-character words: mean of the merged weight, mimo-tc weight + offset (words both
    mimo-sc and mimo-tc have) and the count of the word in the pair sources on the same scale
    (see count_scale). Single characters: mean of the merged weight and the count on the same scale.
    The count is of the text, whatever its reading: the most likely reading of a text is blended
    and its other readings move by the same amount, so their order stays.
    Taiwan and Hong Kong words from OpenCC (regional_tw_hk): at least mimo-tc weight + offset - 350.
    Reviewed overlay rows and the names (`fixed`) keep their weight; `upward_only` terms only shift up."""
    sc_keys = {(hans, code) for hans, code, _ in load_mimo_sc_dataset()["entries"]}
    regional = tc_dataset().get("regional_tw_hk", {})
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
            delta = (mean * n + counted) / (n + 1) - mean
            shift[text] = max(0.0, delta) if key in upward_only else delta
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


def counted_shift(weights, fixed, count_weight, share, upward_only=frozenset()):
    """Moves every reading of a text towards the count of the text by `share` of the gap between
    the count and the weight of its most likely reading. Reviewed rows and names (`fixed`) are not
    moved, but each stays below the words with the same reading that it was below before."""
    top = {}
    for key, weight in weights.items():
        if key not in fixed and (key[1] not in top or weights[top[key[1]]] < weight): top[key[1]] = key
    shift = {
        text: max(0.0, share * (count_weight(key) - weights[key])) if key in upward_only else share * (count_weight(key) - weights[key])
        for text, key in top.items()
    }
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


def homophone_context_order(context, weights, report):
    """Raises the context value of a word to just above that of every homophone (same letters)
    that the pinyin index ranks below it. Context values are the merged weights, from before the
    blend with the counted text, so they could keep the old order of two spellings (魯肉飯 above
    滷肉飯) and the join bonus would bring it back. Nothing is lowered."""
    groups = collections.defaultdict(dict)
    for (hant, _, code), weight in sorted(weights.items()):
        group = groups[toneless(code).replace(" ", "")]
        group[hant] = max(group.get(hant, 0), weight)
    raised = 0
    # In a fixed order: a word in more than one group is raised once per group.
    for _, group in sorted(groups.items()):
        if len(group) < 2: continue
        below, last_weight, pending = 0, None, 0
        for hant, weight in sorted(group.items(), key=lambda item: (item[1], item[0])):
            if weight != last_weight: below, last_weight = max(below, pending), weight
            pending = max(pending, context.get(hant, 0))
            if context.get(hant, 0) <= below and below > 0:
                context[hant] = min(10000, below + 1); raised += 1
                pending = max(pending, context[hant])
    report["context_raised_over_homophones"] = raised


RARE_READING_MAX_DROP = 3500  # a reading is lowered at most this far below the main one (e^5)


def rare_readings(words, keep, evidence_skip, report):
    """Lower weights for the readings of a character that its words seldom use. Returns key -> weight.

    mimo-tc gives all readings of a character one weight unless McBopomofo ranks them (和 han4 above
    he2 is Taiwan usage), so a rare reading can sit level with or above the common one (洗 xian3
    next to xi3, 见 xian4 next to jian4).

    The share of a reading is how much of the character's use in the lexicon's words (each weighted
    e^(weight/700)) reads it so. The main reading is the one mimo-sc lists for the character, else
    the heaviest. A reading with a smaller share than the main one is placed at most
    700 ln(its share / that share) from it, and never lower than RARE_READING_MAX_DROP below it.
    Only readings that mimo-tc did not rank (same mimo-tc weight as the main reading) and that are
    typed with other letters (toneless input meets every tone of the same letters) are moved, and
    nothing is raised. Rows in `keep` keep their weight; words in `evidence_skip` (names, phrases:
    read with each character's heaviest reading) are not counted."""
    tc_raw = {(hant, hans, code): weight for hant, hans, code, weight in tc_dataset()["entries"] if len(hant) == 1}
    use = collections.defaultdict(float)
    for (hant, hans, code), weight in words.items():
        if len(hans) > 1 and (hant, hans, code) not in evidence_skip:
            for character, sound in zip(hans, code.split()): use[(character, sound)] += math.exp(weight / 700)
    listed = {(hans, code) for hans, code, _ in load_mimo_sc_dataset()["entries"] if len(hans) == 1}
    groups = collections.defaultdict(list)
    for key in words:
        if len(key[1]) == 1: groups[key[1]].append(key)
    lowered = {}
    for character, keys in groups.items():
        if len(keys) < 2: continue
        main = max(keys, key=lambda key: ((key[1], key[2]) in listed, words[key], key))
        main_use = use[(character, main[2])]
        if main_use <= 0: continue
        for key in keys:
            if toneless(key[2]) == toneless(main[2]) or key in keep or use[(character, key[2])] >= main_use: continue
            if key not in tc_raw or tc_raw[key] != tc_raw.get(main): continue
            share = max(use[(character, key[2])] / main_use, math.exp(-RARE_READING_MAX_DROP / 700))
            limit = clamp_i16_weight(words[main] + 700 * math.log(share))
            if limit < words[key]: lowered[key] = limit
    report["rare_readings_lowered_for_pinyin"] = len(lowered)
    return lowered


def main(destination, converter, *options):
    output = pathlib.Path(destination)
    output.mkdir(parents=True, exist_ok=True)
    associations = json.loads((ROOT / "Resources/Lexicon/associations.json").read_text())
    (output / "associations.json").write_text(json.dumps(associations, ensure_ascii=False))
    words, native_keys, merge_report, phrase_counts, name_keys, term_keys, traditional_forms, folded_texts = merge_simplified(imported_words(converter), converter, "--no-phrases" not in options)
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
    # The never-volunteer list (Resources/Lexicon/blocked-terms.txt, scripts/blocked_terms.py). A slogan
    # built on a [zh-drop] term is left out of the lexicon; its words stay. A word with a [zh] term (nv)
    # stays typeable by its full reading but is never volunteered: no initials keys, no context rows
    # (next-word associations and the join bonus), never raised over a homophone, its weight kept below
    # the everyday words with the same letters, and FLAG_NO_SUGGEST in mimo_base.bin.
    dropped = {key for key in words if drop_hit(key[1])}
    for key in dropped:
        del words[key]
    native_keys -= dropped
    nv = {key for key in words if zh_hit(key[1])}
    merge_report["never_volunteer_slogans_dropped"] = len(dropped)
    merge_report["never_volunteer_words"] = len(nv)
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
    count_weight = count_scale(words, reviewed_keys | set(phrase_counts), phrase_counts, merge_report, {**tc_dataset()["remapped_words"], **folded_texts}) if counts else lambda key: None
    for key in phrase_counts:
        if key in words and key not in reviewed_keys: words[key] = count_weight(key)
    zhuyin_weight, tc_raw = zhuyin_weights(words, reviewed_keys, merge_report["traditional_only_scale_offset"])
    # Pinyin is typed to the mainland standard: rare readings of a character go below its usual one.
    # Zhuyin keeps McBopomofo's readings, which come with their own Taiwan heterophony tiers.
    rare = rare_readings(words, reviewed_keys | derived_keys, name_keys | set(phrase_counts), merge_report)
    pinyin_weight = pinyin_weights({**words, **rare}, native_keys, reviewed_keys, name_keys, tc_raw, merge_report["traditional_only_scale_offset"], count_weight, merge_report, upward_only=term_keys)
    if counts:
        zhuyin_weight = counted_shift(zhuyin_weight, reviewed_keys | name_keys, count_weight, ZHUYIN_COUNT_SHARE, upward_only=term_keys)
        zhuyin_weight = spoken_weights(zhuyin_weight, reviewed_keys, merge_report, "zhuyin")
        pinyin_weight = spoken_weights(pinyin_weight, reviewed_keys, merge_report, "pinyin")
    sandhi_tones(zhuyin_weight, tc_raw, reviewed_keys, merge_report)
    def add_word(hant, hans, code, zhuyin, weight):
        sounds = code.split()
        phonetic = "".join(encode(sound) for sound in sounds)
        add("zhuyin", re.sub("[ 6347]", "", phonetic), phonetic, hant, hans, zhuyin)
        aliases = {"".join(sounds)}
        aliases |= {re.sub(r"([nl])ve", r"\1ue", p) for p in list(aliases)}
        # Full pinyin is primary. Initial abbreviations remain available with a lower prior,
        # never for a never-volunteer word.
        if len(sounds) > 1 and not zh_hit(hans):
            for short in abbreviations(sounds):
                add("pinyin", re.sub("[1-5]", "", short), short, hant, hans, max(1, weight // 8))
        for reading in aliases: add("pinyin", re.sub("[1-5]", "", reading), reading, hant, hans, weight)
    for (hant, hans, code), merged_weight in words.items():
        if (hant, hans, code) not in native_keys: continue
        add_word(hant, hans, code, zhuyin_weight[(hant, hans, code)], pinyin_weight[(hant, hans, code)])
        if len(hant) > 1 and not zh_hit(hans): context[hant] = max(context.get(hant, 0), merged_weight)
    # The Traditional spellings merge_simplified folded into their Simplified text (臺 into 台, 臺北
    # into 台北) stay Traditional forms of it, below the kept form by their mimo-sc difference.
    kept_form = {(hans, code): (hant, hans, code) for (hant, hans, code) in words if (hant, hans, code) in native_keys}
    kept_traditional = 0
    for (hant, hans, code), drop in sorted(traditional_forms.items()):
        kept = kept_form.get((hans, code))
        if kept is None or kept[0] == hant: continue
        add_word(hant, hans, code, max(1, zhuyin_weight[kept] - drop), max(1, pinyin_weight[kept] - drop))
        if len(hant) > 1 and kept[0] in context and not zh_hit(hans): context[hant] = max(context.get(hant, 0), context[kept[0]] - drop)
        kept_traditional += 1
    merge_report["traditional_spellings_kept_as_forms"] = kept_traditional
    # A [zh] term that is no lexicon word of its own (一个中国, 去死) is still a never-volunteer word:
    # the decoder builds it from smaller words, and the engine's backstop (Lexicon::volunteers) and
    # mpn_text_no_suggest know the list only by its marked rows. Such a term gets rows of weight 1,
    # with the reading of its split into the fewest lexicon words (the heaviest such split).
    by_hans = {}
    for (hant, hans, code), weight in words.items():
        if (hant, hans, code) in native_keys and (hans not in by_hans or by_hans[hans][1] < weight):
            by_hans[hans] = ((hant, code), weight)
    have = {hans for (_, _, _, hans) in rows["pinyin"]}
    term_rows = 0
    for term in zh_terms():
        if len(term) < 2 or term in have:
            continue
        best = [None] * (len(term) + 1)
        best[0] = (0, 0, ())
        for i in range(len(term)):
            if best[i] is None: continue
            for j in range(i + 1, len(term) + 1):
                piece = by_hans.get(term[i:j])
                if piece is None: continue
                split = (best[i][0] + 1, best[i][1] - piece[1], best[i][2] + (piece[0],))
                if best[j] is None or split[:2] < best[j][:2]: best[j] = split
        if best[-1] is None: raise ValueError(f"blocked-terms.txt: no reading for {term}")
        pieces = best[-1][2]
        add_word("".join(hant for hant, _ in pieces), term, " ".join(code for _, code in pieces), 1, 1)
        term_rows += 1
    merge_report["never_volunteer_terms_given_rows"] = term_rows
    homophone_context_order(context, {key: pinyin_weight[key] for key in native_keys if len(key[0]) > 1 and key not in nv}, merge_report)
    # The merge keeps one Traditional form per (Simplified, reading). Zhuyin output is Traditional
    # only, so it also gets the other mimo-tc forms (讚 next to 贊, 準 next to 准). A word gets its
    # mimo-tc weight + offset; a single character is placed below the kept form by its mimo-tc
    # difference, because mimo-tc gives all readings of a character one weight.
    offset = merge_report["traditional_only_scale_offset"]
    kept = {(hans, code): (hant, zhuyin_weight[(hant, hans, code)]) for (hant, hans, code) in words if (hant, hans, code) in native_keys}
    zhuyin_have = {(hant, hans, reading) for (_, reading, hant, hans) in rows["zhuyin"]}
    extra_forms = 0
    for (hant, hans, code), raw in tc_raw.items():
        if len(hant) > 4 or raw < ZHUYIN_EXTRA_FORM_MIN_WEIGHT or drop_hit(hans): continue
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
    # Next-word associations are cut from the context rows, so no row may hold a never-volunteer
    # term: not the word, not a phrase around it, not one that completes it from its first character.
    # The rows are Traditional: checked in Simplified.
    phrases_hant = sorted(context)
    phrases_hans = subprocess.run([converter], input="\n".join(phrases_hant) + "\n", text=True, capture_output=True, check=True).stdout.splitlines()
    if len(phrases_hans) != len(phrases_hant): raise ValueError("Simplified conversion mismatch (context)")
    left_out = [hant for hant, hans in zip(phrases_hant, phrases_hans) if zh_hit(hans) or zh_hit(hant)]
    for hant in left_out: del context[hant]
    merge_report["never_volunteer_context_rows_left_out"] = len(left_out)
    sorted_context = sorted(context.items())
    # English words inside pinyin (scripts/mimo_latin.py): upper-case keys, so the Japanese side never sees them.
    for key, weight in latin_rows(): rows["pinyin"][key] = weight
    merge_report["latin_words"] = len(latin_rows())
    # Never-volunteer rows (by text: also the kept Traditional forms and the zhuyin-only forms; English
    # inside pinyin by the [en] list) sit below the heaviest other word with the same letters.
    def never(hant, hans):
        return zh_hit(hans) or (hant.isascii() and en_hit(hant))
    no_suggest = {(hant, hans) for entries in rows.values() for (_, _, hant, hans) in entries if never(hant, hans)}
    whole_only = {(hant, hans) for hant, hans in no_suggest if zh_whole_only(hans)}
    # [zh-keep] words: the engine does not count the listed term inside them (印度支那 by its initials).
    keep = set(blocked_sections()["zh-keep"])
    keep_rows = {(hant, hans) for entries in rows.values() for (_, _, hant, hans) in entries if hans in keep}
    merge_report["listed_keep_words"] = len(keep_rows)
    capped = 0
    for mode, entries in rows.items():
        best = {}
        for (bare, reading, hant, hans), weight in entries.items():
            if (hant, hans) not in no_suggest and (mode == "zhuyin" or _is_fully_spelled_reading(reading.encode())):
                best[bare] = max(best.get(bare, 0), weight)
        for key, weight in entries.items():
            if (key[2], key[3]) in no_suggest and key[0] in best and weight >= best[key[0]]:
                entries[key] = max(1, best[key[0]] - 1)
                capped += 1
    merge_report["never_volunteer_rows"] = sum(1 for entries in rows.values() for key in entries if (key[2], key[3]) in no_suggest)
    merge_report["never_volunteer_rows_capped_below_homophone"] = capped
    latin_keys = {key for key, _ in latin_rows()}
    rows_without_latin = sorted(((key, weight) for key, weight in rows["pinyin"].items() if key not in latin_keys),
                                key=lambda row: (row[0][0], -row[1], row[0][1:]))
    sorted_rows = {}
    for mode, entries in rows.items():
        ordered = sorted(entries.items(), key=lambda row: (row[0][0], -row[1], row[0][1:]))
        sorted_rows[mode] = ordered
        with (output / f"native-{mode}.tsv").open("w") as dest:
            dest.write(f"MEOWPUUT-{mode.upper()}-1\n")
            for (bare, reading, hant, hans), weight in ordered:
                flag = "\tw" if (hant, hans) in whole_only else "\tn" if (hant, hans) in no_suggest else "\tk" if (hant, hans) in keep_rows else ""
                dest.write(f"{bare}\t{reading}\t{hant}\t{hans}\t{weight}{flag}\n")
            dest.write("# context\n")
            for word, weight in sorted_context: dest.write(f"{word}\t{weight}\n")
    bin_stats = write_mimo_binaries(output, sorted_rows["pinyin"], sorted_rows["zhuyin"], sorted_context, frozenset(no_suggest), frozenset(whole_only), frozenset(keep_rows))
    merge_report.update(bin_stats)
    if counts: merge_report.update(write_pair_table(output, sorted_rows["pinyin"], sorted_rows["zhuyin"],
                                                    no_suggest=frozenset(hans for _, hans in no_suggest)))
    japanese = {}
    def add_japanese(reading, word, weight):
        if not re.fullmatch("[a-z'-]+", reading) or not word or not 0 < weight <= 10000:
            raise ValueError(f"invalid Japanese entry: {reading!r}, {word!r}, {weight}")
        # The pool offered inside pinyin typing: never a word of the never-volunteer list.
        if ja_hit(word):
            raise ValueError(f"Japanese entry {word!r} is on the never-volunteer list (Resources/Lexicon/blocked-terms.txt)")
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
    write_evidence_lexicon(db, words, zhuyin_weight)
    def record(language, reading, word, weight):
        db.execute("INSERT INTO words VALUES(?,?,?,?) ON CONFLICT DO UPDATE SET weight=max(weight,excluded.weight)", (language, reading, word, weight))
    for reading, items in japanese.items():
        for item in items: record("ja", reading.replace("'", ""), item["text"], item["weight"])
    write_zhuyin_initials(db)
    db.commit()
    db.execute("VACUUM")
    db.close()
    print(f"Meowpuut overlay: {authored_count} Chinese rows; {len(japanese)} Japanese readings; total {len(words):,} Chinese readings")


if __name__ == "__main__":
    main(*sys.argv[1:])
