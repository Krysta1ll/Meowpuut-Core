"""Compile the bundled word data into the portable engine's UTF-8 table."""
import collections
import functools
import pathlib
import shutil
import re
import sys
import json
import sqlite3
import hashlib
import math
import unicodedata
import statistics
from japanese_readings import pinyin_kana

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

_BOPOMOFO_KEYS = dict(zip(
    "ㄅㄆㄇㄈㄉㄊㄋㄌㄍㄎㄏㄐㄑㄒㄓㄔㄕㄖㄗㄘㄙㄧㄨㄩㄚㄛㄜㄝㄞㄟㄠㄡㄢㄣㄤㄥㄦ",
    "1qaz2wsxedcrfv5tgbyhnujm8ik,9ol.0p;/-",
))
_BOPOMOFO_TONES = {"ˊ": "6", "ˇ": "3", "ˋ": "4", "˙": "7"}


def source_bopomofo_keys(reading):
    """Return the source's Daqian reading, including the implicit first tone."""
    if not reading:
        return None
    tone = _BOPOMOFO_TONES.get(reading[-1], " ")
    sounds = reading[:-1] if reading[-1] in _BOPOMOFO_TONES else reading
    if not sounds or any(char not in _BOPOMOFO_KEYS for char in sounds):
        return None
    return "".join(_BOPOMOFO_KEYS[char] for char in sounds) + tone


def inspect_source_reading(bopomofo, pinyin, physical_keys):
    """Compare independent source columns; an incorrect key column is diagnostic only."""
    source = source_bopomofo_keys(bopomofo)
    generated = encode(pinyin)
    issues = []
    if source is None:
        issues.append("invalid_bopomofo")
    if generated is None:
        issues.append("unencodable_pinyin")
    if source is not None and generated is not None and source != generated:
        issues.append("tone_conflict" if source[:-1] == generated[:-1] else "sound_conflict")
    if source is not None and physical_keys != (source[:-1] if source.endswith(" ") else source):
        issues.append("key_conflict")
    return source, generated, issues

def canonical_pinyin(syllable):
    # Pinyin sources commonly write nüe/lüe without the umlaut; both forms
    # represent ㄋㄩㄝ/ㄌㄩㄝ, not the unrelated finals ㄋㄨㄜ/ㄌㄨㄜ.
    return re.sub(r"^([nl])ue([1-5])$", r"\1ve\2", syllable)


@functools.lru_cache(maxsize=None)
def encode(syllable):
    syllable = canonical_pinyin(syllable)
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
    for number, line in enumerate((ROOT / "Resources/Lexicon" / name).read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"): continue
        fields = line.split("|")
        yield number, fields



def convert_script(_converter, words, traditional=False):
    # One pinned, reviewable conversion table for every platform. The optional
    # converter argument remains accepted for older build scripts.
    from frozen_normalization import convert
    return convert(words, traditional)


def upstream_checksum(path):
    # Git for Windows may check out the pinned LF source data with CRLF.
    return hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def imported_words(converter, audit=None):
    root = ROOT / "DataSources/McBopomofo"
    manifest = json.loads((root / "SOURCE.json").read_text(encoding="utf-8"))
    for name, checksum in manifest["files"].items():
        if upstream_checksum(root / name) != checksum:
            raise ValueError(f"Upstream data changed: {name}; put edits in Resources/Lexicon instead")
    exclusions = {}
    excluded_ids = set()
    reviewed = manifest.get("reading_exclusions")
    if reviewed:
        path = ROOT / reviewed["file"]
        if upstream_checksum(path) != reviewed["sha256"]:
            raise ValueError("Reviewed source reading exclusions checksum mismatch")
        data = json.loads(path.read_text(encoding="utf-8"))
        if data.get("version") != 1 or not isinstance(data.get("rules"), list):
            raise ValueError("Invalid source reading exclusions format")
        ids = set()
        for rule in data["rules"]:
            if not isinstance(rule, dict) or any(not isinstance(rule.get(k), str) or not rule[k]
                    for k in ("id", "source", "source_line", "word", "pinyin", "reason", "evidence")):
                raise ValueError("Invalid source reading exclusion rule")
            fields = rule["source_line"].split()
            if (rule["source"] != "McBopomofo/BPMFBase.txt" or len(fields) < 4 or
                    fields[0] != rule["word"] or canonical_pinyin(fields[2].lower().replace("ü", "v")) != rule["pinyin"] or
                    rule["id"] in ids or rule["source_line"] in exclusions):
                raise ValueError("Invalid or duplicate source reading exclusion identity")
            ids.add(rule["id"])
            exclusions[rule["source_line"]] = rule
    reviewed_matches = []
    frequency = {}
    for line in (root / "phrase.occ").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if len(fields) == 2: frequency[fields[0]] = int(fields[1])
    mapping = {}
    entries = {}
    skipped = 0
    counts = collections.Counter()
    issues = []
    # Establish reading aliases using the upstream explicit per-character pinyin.
    for number, line in enumerate((root / "BPMFBase.txt").read_text(encoding="utf-8").splitlines(), 1):
        fields = line.split()
        if len(fields) < 4: continue
        word, bpmf, pinyin, physical_keys = fields[:4]
        pinyin = pinyin.lower().replace("ü", "v")
        if not pinyin[-1:].isdigit(): pinyin += "1"
        if bpmf.startswith("ㄝ"): pinyin = "eh" + pinyin[-1]
        pinyin = canonical_pinyin(pinyin)
        source, generated, row_issues = inspect_source_reading(bpmf, pinyin, physical_keys)
        counts.update(row_issues)
        if row_issues:
            issues.append({"line": number, "word": word, "bopomofo": bpmf, "pinyin": pinyin,
                           "source_keys": physical_keys, "expected_keys": source,
                           "generated_keys": generated, "issues": row_issues})
        rule = exclusions.get(line)
        if rule:
            excluded_ids.add(rule["id"])
            reviewed_matches.append({"id": rule["id"], "line": number, "word": word,
                                     "pinyin": pinyin, "evidence": rule["evidence"]})
            continue
        if generated is None:
            skipped += 1
            continue
        # The key column can contain a typo even when the two independent
        # readings agree. Never propagate a conflicting reading into phrases.
        if source == generated:
            mapping.setdefault(bpmf, pinyin)
        if any(ord(char) < 0x3400 for char in word): continue
        entries[(word, pinyin)] = max(1, round(700 * math.log1p(frequency.get(word, 0))))
    unverified_phrases = []
    for number, line in enumerate((root / "BPMFMappings.txt").read_text(encoding="utf-8").splitlines(), 1):
        fields = line.split()
        if len(fields) < 2: continue
        word, syllables = fields[0], fields[1:]
        if len(syllables) > 12 or any(part not in mapping for part in syllables):
            missing = sorted({part for part in syllables if part not in mapping})
            if missing:
                unverified_phrases.append({"line": number, "word": word, "missing_readings": missing})
            skipped += 1
            continue
        code = " ".join(mapping[part] for part in syllables)
        if len(word) != len(syllables):
            skipped += 1
            continue
        entries[(word, code)] = max(1, round(700 * math.log1p(frequency.get(word, 0))))
    # Preserve separately reviewed entries removed by an upstream regional
    # cleanup, with their original occurrence counts and MIT provenance.
    retained = manifest.get("retained_phrases")
    retained_rows = 0
    if retained:
        retained_path = ROOT / retained["file"]
        if upstream_checksum(retained_path) != retained["sha256"]:
            raise ValueError("Retained phrase checksum mismatch")
        seen = set()
        for number, line in enumerate(retained_path.read_text(encoding="utf-8").splitlines(), 1):
            if not line or line.startswith("#"):
                continue
            fields = line.split("|")
            if len(fields) != 3 or not fields[2].isdigit():
                raise ValueError(f"Retained phrases:{number}: invalid row")
            word, bpmf, occurrence = fields
            syllables = bpmf.split()
            if (not word or len(word) != len(syllables) or len(syllables) > 12 or
                    any(part not in mapping for part in syllables)):
                raise ValueError(f"Retained phrases:{number}: invalid or unverified reading")
            code = " ".join(mapping[part] for part in syllables)
            if (word, code) in seen:
                raise ValueError(f"Retained phrases:{number}: duplicate entry")
            seen.add((word, code))
            # If upstream reintroduces the same phrase, its current occurrence
            # wins; otherwise retain the original source's weight formula.
            entries.setdefault((word, code), max(1, round(700 * math.log1p(int(occurrence)))))
            retained_rows += 1
        if retained_rows != retained["rows"]:
            raise ValueError("Retained phrase count mismatch")
    # A common reading gets the full character prior; other readings are discounted.
    main_reading = {}
    for line in (root / "heterophony1.list").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1] in mapping: main_reading[fields[0]] = mapping[fields[1]]
    for key in entries:
        word, code = key
        if len(word) == 1 and word in main_reading and main_reading[word] != code:
            entries[key] = max(1, entries[key] // 8)
    text = sorted({word for word, _ in entries})
    converted = convert_script(converter, text)
    if len(converted) != len(text): raise ValueError("Script conversion count mismatch")
    hans = dict(zip(text, converted))
    missing_exclusions = sorted(rule["id"] for rule in exclusions.values() if rule["id"] not in excluded_ids)
    if audit is not None:
        audit.update({"source": "McBopomofo/BPMFBase.txt", "retained_phrase_rows": retained_rows, "issue_counts": dict(sorted(counts.items())),
                      "reviewed_exclusions": {"matched": reviewed_matches, "missing_rule_ids": missing_exclusions},
                      "issues": issues, "unmapped_phrase_rows": len(unverified_phrases),
                      "unmapped_phrases": unverified_phrases})
    print("Source reading audit:", json.dumps({"issue_counts": dict(sorted(counts.items())),
          "reviewed_exclusions": len(reviewed_matches), "missing_rule_ids": missing_exclusions,
          "unmapped_phrase_rows": len(unverified_phrases)}, ensure_ascii=False, sort_keys=True))
    print(f"MIT source: {len(entries):,} accepted readings; {skipped} unsupported/mismatched rows excluded")
    return {(word, hans[word], code): weight for (word, code), weight in entries.items()}

def numbered_pinyin(syllable):
    result, tone = "", "5"
    for char in unicodedata.normalize("NFD", syllable.lower()):
        if char in {"\u0304", "\u0301", "\u030c", "\u0300"}:
            tone = {"\u0304":"1", "\u0301":"2", "\u030c":"3", "\u0300":"4"}[char]
        elif char == "\u0308" and result.endswith("u"): result = result[:-1] + "v"
        elif char == "\u0302" and result.endswith("e"): result += "h"
        else: result += char
    return canonical_pinyin(result + tone)

def merge_simplified(words, converter, simplified_scores=None):
    root = ROOT / "DataSources/PinyinPro"
    manifest = json.loads((root / "SOURCE.json").read_text(encoding="utf-8"))
    for name, expected in manifest["files"].items():
        if upstream_checksum(root / name) != expected:
            raise ValueError(f"Simplified source checksum mismatch: {name}")
    source = json.loads((root / "complete.json").read_text(encoding="utf-8"))
    canonical = {}
    for (hant, hans, reading), weight in words.items():
        if (hans, reading) not in canonical or canonical[(hans, reading)][1] < weight:
            canonical[(hans, reading)] = (hant, weight)
    merged = {(hant, hans, reading): weight for (hans, reading), (hant, weight) in canonical.items()}
    native_keys = set(merged)
    valid, skipped = [], 0
    for hans, values in source.items():
        if not 1 <= len(hans) <= 12 or not all(0x3400 <= ord(c) <= 0x9fff for c in hans): skipped += 1; continue
        reading = " ".join(numbered_pinyin(part) for part in values[0].split())
        if len(reading.split()) != len(hans) or not all(encode(x) for x in reading.split()): skipped += 1; continue
        valid.append((hans, reading, float(values[1])))
    source_valid_count = len(valid)
    # Some PinyinPro keys are traditional single characters. Remove a
    # duplicate only when both sources agree on reading and the simplified
    # form roundtrips to the original; ambiguous mappings stay untouched.
    pinyin_single = {(word, reading) for word, reading, _ in valid if len(word) == 1}
    candidate_pairs = collections.defaultdict(set)
    for hant, hans, reading in words:
        if (len(hant) == len(hans) == 1 and hant != hans and
                (hant, reading) in pinyin_single and (hans, reading) in pinyin_single):
            candidate_pairs[(hant, reading)].add(hans)
    candidates = {key: next(iter(values)) for key, values in candidate_pairs.items() if len(values) == 1}
    targets = sorted(set(candidates.values()))
    reverse = convert_script(converter, targets, traditional=True) if targets else []
    if len(reverse) != len(targets): raise ValueError("Single-character reverse conversion mismatch")
    reverse_by_target = dict(zip(targets, reverse))
    traditional_source_aliases = {key for key, hans in candidates.items() if reverse_by_target[hans] == key[0]}
    valid = [(word, reading, frequency) for word, reading, frequency in valid
             if (word, reading) not in traditional_source_aliases]
    missing = sorted({hans for hans, reading, _ in valid if (hans, reading) not in canonical})
    converted = convert_script(converter, missing, traditional=True)
    if len(converted) != len(missing): raise ValueError("Traditional conversion mismatch")
    traditional = dict(zip(missing, converted))
    # PinyinPro includes traditional forms as standalone source keys. Only
    # promote rare single characters when the configured converter confirms
    # the key is already simplified, so this does not pollute simplified output.
    low_single = sorted({hans for hans, reading, frequency in valid
                         if len(hans) == 1 and frequency < 1e-11 and (hans, reading) not in canonical})
    simplified_single = convert_script(converter, low_single) if low_single else []
    if len(simplified_single) != len(low_single): raise ValueError("Single-character conversion mismatch")
    eligible_low_single = {original for original, converted in zip(low_single, simplified_single) if original == converted}
    # Align the two corpus scales using shared readings, then modestly prefer
    # simplified-source usage. These are ranking priors, not comparable raw counts.
    def source_weight(frequency):
        return max(1, round(700 * math.log1p(max(0, frequency) * 1e12)))
    differences = [source_weight(frequency) - canonical[(hans, reading)][1]
        for hans, reading, frequency in valid
        if (hans, reading) in canonical and frequency > 0 and canonical[(hans, reading)][1] > 0]
    offset = round(statistics.median(differences)) if differences else 0
    merged = {key: max(1, weight + offset - 350) for key, weight in merged.items()}
    duplicates = added = 0
    for hans, reading, frequency in valid:
        existing = canonical.get((hans, reading))
        hant = existing[0] if existing else traditional[hans]
        key = (hant, hans, reading)
        # Simplified-source frequency wins a duplicate; our reviewed overlay applies last.
        merged[key] = source_weight(frequency)
        if simplified_scores is not None:
            simplified_scores[(hans, reading)] = merged[key]
        duplicates += existing is not None
        added += existing is None
        # A single character must remain selectable on its own even if its
        # corpus frequency is too low to put a whole phrase in the hot index.
        if existing or frequency >= 1e-11 or hans in eligible_low_single: native_keys.add(key)
    # Simplified spelling is not a unique key for a traditional word:
    # 志／誌, 周／週 and 記錄／紀錄 can share simplified spelling and reading.
    # Keep upstream traditional forms alongside the preferred simplified
    # entry; the engine deduplicates identical displayed simplified text.
    for key, weight in words.items():
        if key not in merged:
            merged[key] = max(1, weight + offset - 350)
            native_keys.add(key)
    report = {"name": "mimo", "version": "1.0", "simplified_rows": source_valid_count,
              "traditional_source_aliases_removed": len(traditional_source_aliases), "duplicate_readings_merged": duplicates,
              "new_readings": added, "excluded_invalid_rows": skipped,
              "merged_readings": len(merged), "native_hot_readings": len(native_keys),
              "traditional_only_scale_offset": offset, "traditional_only_preference_discount": 350,
              "note": "All valid new readings remain in indexed SQLite; frequent words also enter native segmentation."}
    print("Simplified merge:", json.dumps(report, ensure_ascii=False))
    return merged, native_keys, report

def apply_reviewed_simplified_exclusions(words, native_keys, protected_keys=()):
    """Remove reviewed source variants only when the intended alternative exists."""
    protected_keys = set(protected_keys)
    exclusions = []
    seen = set()
    for number, fields in authored("reviewed-simplified-exclusions.tsv"):
        if len(fields) != 4:
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: expected 4 fields")
        hant, compact_reading, bad, good = fields
        sounds = re.findall(r"[a-züê]+[1-5]", compact_reading)
        if (not hant or not bad or not good or bad == good or
                "".join(sounds) != compact_reading or len(sounds) != len(hant) or
                not all(encode(sound) for sound in sounds)):
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: invalid rule")
        reading = " ".join(sounds)
        bad_key, good_key = (hant, bad, reading), (hant, good, reading)
        if bad_key in seen:
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: duplicate rule")
        seen.add(bad_key)
        if good_key not in words:
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: correct alternative missing")
        if bad_key not in words:
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: source variant missing; review rule")
        if bad_key in protected_keys:
            raise ValueError(f"reviewed-simplified-exclusions.tsv:{number}: conflicts with reviewed overlay")
        exclusions.append(bad_key)
    for bad_key in exclusions:
        del words[bad_key]
        native_keys.discard(bad_key)
    return len(exclusions)

def normalize_simplified_pronouns(words, native_keys, protected_keys=()):
    """Use 你 for the aligned ni3 female pronoun in simplified display only.

    妳 has other readings; no other character or unaligned phrase is changed.
    Existing correct entries keep their own source weight.  The result records
    every re-keying so corpus updates can be reviewed without guessing.
    """
    protected = set(protected_keys)
    changes = []
    for (hant, hans, reading), weight in words.items():
        if '妳' not in hans:
            continue
        sounds = reading.split()
        if len(hant) != len(hans) or len(hans) != len(sounds):
            continue
        positions = [index for index, char in enumerate(hans) if char == '妳']
        if not all(hant[index] == '妳' and sounds[index] == 'ni3' for index in positions):
            continue
        old = (hant, hans, reading)
        if old in protected:
            raise ValueError('simplified pronoun rule conflicts with reviewed overlay')
        converted = ''.join('你' if index in positions else char for index, char in enumerate(hans))
        new = (hant, converted, reading)
        changes.append((old, new, weight, words.get(new)))
    for old, new, weight, existing_weight in changes:
        del words[old]
        if existing_weight is None:
            words[new] = max(words.get(new, weight), weight)
        if old in native_keys:
            native_keys.discard(old)
            native_keys.add(new)
    return [{"traditional": old[0], "reading": old[2], "old_simplified": old[1],
             "simplified": new[1], "old_weight": weight,
             "retained_existing_weight": existing_weight}
            for old, new, weight, existing_weight in changes]

def reviewed_simplified_priorities(words, source_scores, protected_keys=()):
    """Validate all simplified-only editorial scores before applying any."""
    protected = set(protected_keys)
    available = {(key[1], key[2]) for key in words}
    protected_pairs = {(key[1], key[2]) for key in protected}
    result = {}
    for number, fields in authored("simplified-priorities.tsv"):
        if len(fields) != 3:
            raise ValueError(f"simplified-priorities.tsv:{number}: expected 3 fields")
        hans, reading, priority = fields
        sounds = reading.split()
        if (not hans or len(hans) != len(sounds) or not all(encode(sound) for sound in sounds)
                or not priority.isdigit() or not 1 <= int(priority) <= 10000):
            raise ValueError(f"simplified-priorities.tsv:{number}: invalid rule")
        pair = (hans, reading)
        if pair in result:
            raise ValueError(f"simplified-priorities.tsv:{number}: duplicate rule")
        if pair not in available or pair not in source_scores:
            raise ValueError(f"simplified-priorities.tsv:{number}: exact source reading missing")
        if pair in protected_pairs:
            raise ValueError(f"simplified-priorities.tsv:{number}: conflicts with reviewed overlay")
        result[pair] = int(priority)
    return result

def main(destination, converter=None):
    output = pathlib.Path(destination)
    output.mkdir(parents=True, exist_ok=True)
    associations = json.loads((ROOT / "Resources/Lexicon/associations.json").read_text(encoding="utf-8"))
    (output / "associations.json").write_text(json.dumps(associations, ensure_ascii=False), encoding="utf-8")
    shutil.copyfile(ROOT / "Resources/Lexicon/emoji-cldr.json", output / "emoji-cldr.json")
    source_audit = {}
    imported = imported_words(converter, source_audit)
    simplified_scores = {}
    words, native_keys, merge_report = merge_simplified(imported, converter, simplified_scores)
    (output / "source-reading-audit.json").write_text(json.dumps(source_audit, ensure_ascii=False, indent=2), encoding="utf-8")
    (output / "lexicon-merge-report.json").write_text(json.dumps(merge_report, indent=2))
    authored_count = 0
    reviewed_groups = {}
    def group(key):
        hant, hans, code = key
        # Distinct traditional spellings remain selectable, including phrases.
        return (hans, code, hant)
    derived_keys = set()
    authored_rows = list(authored("chinese.tsv"))
    # A phrase's conversion can depend on context (紀錄 → 记录), so its
    # individual character pairs are only safe when supported independently.
    character_pairs = {(hant, hans) for hant, hans, _ in imported if len(hant) == len(hans) == 1}
    character_pairs.update((fields[0], fields[1]) for _, fields in authored_rows
                           if len(fields) == 4 and len(fields[0]) == len(fields[1]) == 1)
    for number, fields in authored_rows:
        if len(fields) != 4: raise ValueError(f"chinese.tsv:{number}: expected 4 fields")
        traditional, simplified, code, priority = fields
        sounds = code.split()
        if not all(encode(s) for s in sounds): raise ValueError(f"chinese.tsv:{number}: invalid pinyin")
        if not traditional or not simplified or not priority.isdigit(): raise ValueError(f"chinese.tsv:{number}: invalid word")
        weight = int(priority)
        reviewed_groups[group((traditional, simplified, code))] = ((traditional, simplified, code), weight)
        words[(traditional, simplified, code)] = weight
        native_keys.add((traditional, simplified, code))
        authored_count += 1
        # Character readings are derived only from our own explicitly supplied words.
        if len(traditional) == len(simplified) == len(sounds):
            for hant, hans, sound in zip(traditional, simplified, sounds):
                if hant != hans and (hant, hans) not in character_pairs:
                    continue
                key = (hant, hans, sound)
                words[key] = max(words.get(key, 0), min(1000, weight // 10))
                native_keys.add(key)
                derived_keys.add(key)
    # Run after reviewed entries so that a source variant can be removed only
    # when its reviewed simplified counterpart exists, including 妳→你.
    merge_report["reviewed_simplified_exclusions"] = apply_reviewed_simplified_exclusions(
        words, native_keys, (key for key, _ in reviewed_groups.values()))
    canonical = {}
    native_groups = {group(key) for key in native_keys}
    for key, weight in words.items():
        identity = group(key)
        if identity not in canonical or canonical[identity][1] < weight: canonical[identity] = (key, weight)
    # Reviewed spellings/weights win even when lower than an upstream variant.
    canonical.update(reviewed_groups)
    merge_report["reviewed_readings"] = len(reviewed_groups)
    merge_report["derived_character_readings"] = len(derived_keys)
    merge_report["final_duplicate_variants_removed"] = len(words) - len(canonical)
    words = {key: weight for key, weight in canonical.values()}
    native_keys = {key for key in words if group(key) in native_groups}
    pronouns = normalize_simplified_pronouns(words, native_keys,
                                             (key for key, _ in reviewed_groups.values()))
    merge_report["simplified_pronoun_forms_normalized"] = len(pronouns)
    (output / "simplified-pronoun-audit.json").write_text(
        json.dumps(pronouns, ensure_ascii=False, indent=2), encoding="utf-8")
    merge_report["final_readings"] = len(words)
    merge_report["final_native_readings"] = len(native_keys)
    # Keep the previous shared weight as the traditional prior. Simplified
    # uses an exact source spelling and tone when available; explicit local
    # entries remain reviewed overrides in both scripts.
    reviewed_keys = {key for key, _ in reviewed_groups.values()}
    simplified_priorities = reviewed_simplified_priorities(words, simplified_scores, reviewed_keys)
    hans_weights = {key: weight if key in reviewed_keys else simplified_priorities.get(
                    (key[1], key[2]), simplified_scores.get((key[1], key[2]), weight))
                    for key, weight in words.items()}
    merge_report["simplified_source_weight_readings"] = sum(
        key not in reviewed_keys and (key[1], key[2]) in simplified_scores and
        (key[1], key[2]) not in simplified_priorities for key in words)
    merge_report["simplified_editorial_priority_pairs"] = len(simplified_priorities)
    merge_report["simplified_editorial_weight_readings"] = sum(
        (key[1], key[2]) in simplified_priorities for key in words)
    (output / "lexicon-merge-report.json").write_text(json.dumps(merge_report, indent=2))
    normalization = json.loads((ROOT / "DataSources/MimoNormalization/SOURCE.json").read_text(encoding="utf-8"))
    (output / "mimo-manifest.json").write_text(json.dumps({
        "name": "mimo", "version": "1.0", "readings": len(words),
        "key": "simplified spelling + tone-qualified reading + traditional spelling",
        "frequency_preference": "independent traditional and simplified priors; reviewed overlays last",
        "native_table_version": 2,
        "simplified_editorial_priorities": {"pairs": len(simplified_priorities),
            "sha256": upstream_checksum(ROOT / "Resources/Lexicon/simplified-priorities.tsv")},
        "normalization": {"icu_version": normalization["icu_version"], "snapshot_sha256": normalization["snapshot_sha256"],
                          "snapshot_content_sha256": normalization["snapshot_content_sha256"],
                          "overrides_sha256": normalization["overrides_sha256"]},
        "sources": ["McBopomofo (MIT)", "Pinyin Pro complete (MIT)"],
        "japanese_sources": ["UniDic-CWJ 2025.12 reviewed subset (BSD-3-Clause)", "Meowpuut authored"]
    }, ensure_ascii=False, indent=2))
    rows = {"zhuyin": {}, "pinyin": {}}
    def add(mode, bare, reading, hant, hans, weight, hans_weight):
        key = (bare, reading, hant, hans)
        previous = rows[mode].get(key, (0, 0))
        rows[mode][key] = (max(previous[0], weight), max(previous[1], hans_weight))
    context = {}
    for (hant, hans, code), weight in words.items():
        if (hant, hans, code) not in native_keys: continue
        hans_weight = hans_weights[(hant, hans, code)]
        sounds = code.split()
        phonetic = "".join(encode(sound) for sound in sounds)
        add("zhuyin", re.sub("[ 6347]", "", phonetic), phonetic, hant, hans, weight, hans_weight)
        aliases = {"".join(sounds)}
        aliases |= {re.sub(r"([nl])ve", r"\1ue", p) for p in list(aliases)}
        # Full pinyin is primary. Initial abbreviations remain available with a lower prior.
        if len(sounds) > 1:
            short = "".join(s[0] + s[-1] for s in sounds)
            add("pinyin", re.sub("[1-5]", "", short), short, hant, hans,
                max(1, weight // 8), max(1, hans_weight // 8))
        for reading in aliases: add("pinyin", re.sub("[1-5]", "", reading), reading, hant, hans, weight, hans_weight)
        if len(hant) > 1:
            previous = context.get(hant, (0, 0))
            context[hant] = (max(previous[0], weight), max(previous[1], hans_weight))
    # Context-only observations steer adjacent word segments without creating
    # artificial dictionary entries or changing the standalone word order.
    for number, fields in authored("context.tsv"):
        if len(fields) != 2: raise ValueError(f"context.tsv:{number}: expected 2 fields")
        phrase, priority = fields
        if not 2 <= len(phrase) <= 5 or not all('\u3400' <= c <= '\u9fff' for c in phrase) or not priority.isdigit() or not 1 <= int(priority) <= 10000:
            raise ValueError(f"context.tsv:{number}: invalid phrase or priority")
        previous = context.get(phrase, (0, 0))
        context[phrase] = (max(previous[0], int(priority)), max(previous[1], int(priority)))
    for mode, entries in rows.items():
        with (output / f"native-{mode}.tsv").open("w", encoding="utf-8", newline="\n") as dest:
            dest.write(f"MEOWPUUT-{mode.upper()}-2\n")
            for (bare, reading, hant, hans), (weight, hans_weight) in sorted(entries.items(), key=lambda row: (row[0][0], -row[1][0], row[0][1:])):
                dest.write(f"{bare}\t{reading}\t{hant}\t{hans}\t{weight}\t{hans_weight}\n")
            dest.write("# context\n")
            for word, (weight, hans_weight) in sorted(context.items()):
                dest.write(f"{word}\t{weight}\t{hans_weight}\n")
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
    unidic = ROOT / "DataSources/UniDic"
    manifest = json.loads((unidic / "SOURCE.json").read_text(encoding="utf-8"))
    daily = (unidic / "daily.tsv").read_bytes().replace(b"\r\n", b"\n")
    if hashlib.sha256(daily).hexdigest() != manifest["curated_sha256"]:
        raise ValueError("UniDic curated data changed; update the pinned source record")
    for number, line in enumerate(daily.decode("utf-8").splitlines(), 1):
        if not line or line.startswith("#"): continue
        fields = line.split("|")
        if len(fields) != 5: raise ValueError(f"UniDic daily.tsv:{number}: expected 5 fields")
        kana, word, romaji, chinese, priority = fields
        if not re.fullmatch(r"[ぁ-ゖー]+", kana) or not priority.isdigit():
            raise ValueError(f"UniDic daily.tsv:{number}: invalid reading or priority")
        weight = int(priority)
        if not 4000 < weight <= 10000:
            raise ValueError(f"UniDic daily.tsv:{number}: priority must leave room for a lower-weight alias")
        add_japanese(romaji, word, weight)
        alias = pinyin_kana(word, chinese.split())
        if alias != romaji:
            # Pure-kanji pinyin can be ordinary Chinese; the portable core treats
            # this 3500 weight as ambiguous rather than Japanese-only evidence.
            contains_kana = any("\u3040" <= char <= "\u30ff" for char in word)
            alias_weight = max(4000, weight - 3000) if contains_kana else 3500
            add_japanese(alias, word, alias_weight)
    japanese = {reading: [{"text": word, "weight": weight} for word, weight in entries.items()]
                for reading, entries in japanese.items()}
    for values in japanese.values(): values.sort(key=lambda value: (-value["weight"], value["text"]))
    (output / "native-japanese.json").write_text(json.dumps(japanese, ensure_ascii=False, sort_keys=True), encoding="utf-8")
    # Both platform adapters consume the same merged and deduplicated entries.
    with (output / "native-japanese.tsv").open("w", encoding="utf-8", newline="\n") as dest:
        for reading in sorted(japanese):
            for entry in japanese[reading]:
                dest.write(f"{reading}|{entry['text']}|{entry['weight']}\n")
    database = output / "language-evidence.sqlite"
    if database.exists(): database.unlink()
    db = sqlite3.connect(database)
    db.execute("CREATE TABLE words(language TEXT, reading TEXT, word TEXT, weight INTEGER, PRIMARY KEY(language,reading,word)) WITHOUT ROWID")
    db.execute("CREATE TABLE lexicon(reading TEXT, phonetic TEXT, bare TEXT, hant TEXT, hans TEXT, weight INTEGER, toned TEXT, hans_weight INTEGER NOT NULL, PRIMARY KEY(reading,phonetic,hant,hans)) WITHOUT ROWID")
    def record(language, reading, word, weight):
        db.execute("INSERT INTO words VALUES(?,?,?,?) ON CONFLICT DO UPDATE SET weight=max(weight,excluded.weight)", (language, reading, word, weight))
    for (hant, hans, code), weight in words.items():
        reading = re.sub("[1-5 ]", "", code)
        phonetic = "".join(encode(sound) for sound in code.split())
        db.execute("INSERT OR REPLACE INTO lexicon VALUES(?,?,?,?,?,?,?,?)", (reading, phonetic, re.sub("[ 6347]", "", phonetic), hant, hans, weight, code.replace(" ", ""), hans_weights[(hant, hans, code)]))
    for reading, items in japanese.items():
        for item in items: record("ja", reading.replace("'", ""), item["text"], item["weight"])
    db.execute("CREATE INDEX lexicon_phonetic ON lexicon(phonetic)")
    db.execute("CREATE INDEX lexicon_bare ON lexicon(bare)")
    db.execute("CREATE INDEX lexicon_reading_hans_weight ON lexicon(reading, hans_weight DESC)")
    db.commit(); db.close()
    print(f"Meowpuut overlay: {authored_count} Chinese rows; {len(japanese)} Japanese readings; total {len(words):,} Chinese readings")


if __name__ == "__main__":
    main(*sys.argv[1:])
