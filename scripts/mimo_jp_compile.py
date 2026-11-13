"""Compile step of mimo-jp: snapshot + project files -> mimo_jp.bin.

Called by compile-native-lexicon.py after the Chinese tables are written:
    write_mimo_jp(output, pinyin_rows, japanese, review_dir)

Inputs: DataSources/mimo-jp/mimo-jp.tsv (generated snapshot), DataSources/mimo-jp/kanji-pinyin.tsv
(old forms of kanji), Resources/Lexicon/japanese.tsv and japanese-reviewed.tsv (reviewed words),
japanese-deny.tsv (pairs that must not ship, or not in pinyin mode), japanese-kanji-pinyin.tsv
(readings of polyphonic kanji). The rules are those of DESIGN-mimo-jp.md sections 7 and 8, with the
changes listed in the module constants below.

Writes <output>/mimo_jp.bin, and lists for a reviewer into review_dir (never bundled):
collisions.tsv, authored-unmerged.tsv, alias-polyphones.tsv, deny-unmatched.tsv.
"""
import collections
import hashlib
import json
import pathlib

from japanese_readings import (alias_keys, kana_to_hepburn, load_kanji_table, normalise_key,
                               romaji_to_kana, to_hiragana)
from mimo_jp_binary import (CLASSES, FLAG_PINYIN_OK, FLAG_REVIEWED, KIND_ALIAS_KANA, KIND_ALIAS_KANJI,
                            KIND_AUTHORED, KIND_READING, KIND_VARIANT, REVIEWED_POOL, chinese_build_of,
                            posting_order, read_image, write_image)

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Gate constants (design section 8).
MIN_LETTERS = 3
MIN_PINYIN_LETTERS = 4
W_COLLIDE = 4500
W_PINYIN = 3200
W_ALIAS_KANJI = 3200
SLOT_EXTRA = 2
# Posting weight deductions (design 7.3).
VARIANT_DEDUCTION = 1500
ALIAS_KANA_DEDUCTION = 2000
ALIAS_KANJI_DEDUCTION = 2500
# Mixed pool limits (design 7.5).
MAX_POOL = 512

DENY_REASONS = {"work", "artist", "brand", "name", "collocation", "vulgar", "not-a-word", "pinyin", "hold"}


def is_kanji(ch):
    code = ord(ch)
    return ch == "々" or 0x3400 <= code <= 0x9FFF or 0xF900 <= code <= 0xFAFF or 0x20000 <= code <= 0x323AF


def letters(kana):
    """Letters-only Hepburn of a kana string (`h` of section 8), or None."""
    spelled = kana_to_hepburn(kana)
    return spelled.replace("'", "") if spelled else None


def load_snapshot(path):
    rows = []
    for number, line in enumerate(pathlib.Path(path).read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("|")
        if len(fields) != 8 or not fields[4].isdigit() or fields[5] not in CLASSES:
            raise ValueError(f"mimo-jp.tsv:{number}: expected 8 fields")
        kana, surface, romaji, pinyin, weight, cls, tier, rule = fields
        rows.append({"kana": kana, "surface": surface, "romaji": romaji, "pinyin": pinyin,
                     "weight": int(weight), "class": cls, "tier": tier, "rule": rule})
    return rows


def load_deny(path):
    deny = {}
    path = pathlib.Path(path)
    if not path.exists():
        return deny
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split("|")
        if len(fields) != 3 or fields[2] not in DENY_REASONS or (fields[0], fields[1]) in deny:
            raise ValueError(f"japanese-deny.tsv:{number}: expected kana|surface|reason, once per pair")
        deny[(fields[0], fields[1])] = fields[2]
    return deny


def load_reviewed(root):
    """The 60 reviewed rows: (kana, surface, romaji, pinyin, weight)."""
    rows = []
    for number, line in enumerate((root / "Resources/Lexicon/japanese-reviewed.tsv").read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("|")
        if len(fields) != 5:
            raise ValueError(f"japanese-reviewed.tsv:{number}: expected 5 fields")
        rows.append((fields[0], fields[1], fields[2], fields[3], int(fields[4])))
    return rows


def load_authored(root):
    rows = []
    for number, line in enumerate((root / "Resources/Lexicon/japanese.tsv").read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("|")
        if len(fields) != 3:
            raise ValueError(f"japanese.tsv:{number}: expected 3 fields")
        rows.append((fields[0].split(","), fields[1], int(fields[2])))
    return rows


class ChineseSide:
    """What the gate needs from the merged pinyin table: zc, zw, syllables, pinyin shape."""

    def __init__(self, pinyin_rows):
        self.zc = {}
        self.best = {}
        self.zw = collections.defaultdict(set)
        self.syllables = set()
        for (bare, _reading, hant, hans), weight in pinyin_rows:
            if weight > self.zc.get(bare, 0):
                self.zc[bare] = weight
                self.best[bare] = hant
            self.zw[bare].update((hant, hans))
            if len(hant) == 1:
                self.syllables.add(bare)
        self.prefixes = {s[:i] for s in self.syllables for i in range(1, len(s))}
        self.longest = max((len(s) for s in self.syllables), default=6)
        self._state = {}

    def state(self, h):
        """(shaped, unfinished) of section 8."""
        if h in self._state:
            return self._state[h]
        ok = [True] + [False] * len(h)
        for i in range(len(h)):
            if ok[i]:
                for j in range(i + 1, min(len(h), i + self.longest) + 1):
                    if h[i:j] in self.syllables:
                        ok[j] = True
        shaped = ok[len(h)]
        unfinished = not shaped and any(ok[i] and h[i:] in self.prefixes for i in range(len(h)))
        self._state[h] = (shaped, unfinished)
        return shaped, unfinished

    def pinyin_like(self, h):
        return any(self.state(h))


def passes_reading(h, weight, surface, side, old_forms, variant=False):
    """G1 (and G2 with variant=True) for one derived READING or VARIANT posting."""
    if not h or len(h) < MIN_LETTERS or h in side.syllables:
        return False
    if side.pinyin_like(h) and (len(h) < MIN_PINYIN_LETTERS or weight < W_PINYIN):
        return False
    zc = side.zc.get(h, 0)
    if zc > 0 and (variant or weight < W_COLLIDE):
        return False
    if zc > 0:
        old = "".join(old_forms.get(ch, ch) for ch in surface)
        if surface in side.zw[h] or old in side.zw[h]:
            return False
    return True


def passes_alias(key, kind, weight, surface, side):
    """G3 for ALIAS_KANA, G4 for ALIAS_KANJI.

    G4 differs from the design (owner decision 2026-11-11): a pure-kanji alias is written even
    when a Chinese word has the same reading or the same characters. The client shows it after
    every Chinese candidate, drops it when its text equals a Chinese candidate, and keeps it out
    of the collapsed row when the input is a Chinese reading.
    """
    if len(key) < MIN_PINYIN_LETTERS:
        return False
    if kind == KIND_ALIAS_KANA:
        # Added to G3: a key a Chinese typist passes through needs the weight of rule G1 b.
        return side.zc.get(key, 0) == 0 and (weight >= W_PINYIN or not side.pinyin_like(key))
    return sum(1 for ch in surface if is_kanji(ch)) >= 2 and weight >= W_ALIAS_KANJI


def build(root, pinyin_rows, japanese, side=None):
    """Return (words, postings, reviewed rows, lists for review, counts)."""
    source = root / "DataSources/mimo-jp"
    lexicon = root / "Resources/Lexicon"
    side = side or ChineseSide(pinyin_rows)
    snapshot = load_snapshot(source / "mimo-jp.tsv")
    old_forms = {}
    for line in (source / "kanji-pinyin.tsv").read_text(encoding="utf-8").splitlines():
        fields = line.split("\t")
        if not line.startswith("#") and len(fields) == 4 and fields[3]:
            old_forms[fields[0]] = fields[3]
    table = load_kanji_table(lexicon / "japanese-kanji-pinyin.tsv")
    deny = load_deny(lexicon / "japanese-deny.tsv")
    counts = collections.Counter()
    review = collections.defaultdict(list)

    # Words: snapshot rows minus denied pairs; one word per (normalised kana, surface).
    words, by_pair, by_surface = [], {}, collections.defaultdict(list)
    matched_deny = set()
    for row in snapshot:
        reason = deny.get((row["kana"], row["surface"]))
        if reason:
            matched_deny.add((row["kana"], row["surface"]))
        if reason and reason != "pinyin":
            counts[f"denied_{reason}"] += 1
            continue
        pair = (normalise_key(row["kana"]), row["surface"])
        if pair in by_pair:
            other = words[by_pair[pair]]
            counts["snapshot_duplicate_pairs"] += 1
            if other["weight"] >= row["weight"]:
                continue
            words[by_pair[pair]] = dict(row, reviewed=False, no_pinyin=reason == "pinyin")
            continue
        by_pair[pair] = len(words)
        by_surface[row["surface"]].append(len(words))
        words.append(dict(row, reviewed=False, no_pinyin=reason == "pinyin"))
    review["deny-unmatched"] = [f"{k}|{s}|{r}" for (k, s), r in sorted(deny.items()) if (k, s) not in matched_deny]

    def add_reviewed(kana, surface, weight):
        pair = (normalise_key(kana), surface)
        if pair in by_pair:
            word = words[by_pair[pair]]
            if not word["reviewed"]:
                counts["reviewed_merged"] += 1
                word["weight"] = 0
            word["reviewed"] = True
            word["weight"] = max(word["weight"], weight)
            word["no_pinyin"] = False
            return
        counts["reviewed_own_word"] += 1
        by_pair[pair] = len(words)
        by_surface[surface].append(len(words))
        words.append({"kana": kana, "surface": surface, "romaji": "", "pinyin": "", "weight": weight,
                      "class": "n", "tier": "R", "rule": "", "reviewed": True, "no_pinyin": False})

    for kana, surface, _romaji, _pinyin, weight in load_reviewed(root):
        add_reviewed(kana, surface, weight)
    snapshot_surfaces = collections.defaultdict(list)
    for row in snapshot:
        snapshot_surfaces[row["surface"]].append(row["kana"])
    for aliases, surface, weight in load_authored(root):
        kanas = [romaji_to_kana(alias) for alias in aliases]
        if not any(is_kanji(ch) for ch in surface):
            kana, rule = to_hiragana(surface), 1
        else:
            in_snapshot = snapshot_surfaces.get(surface, [])
            kana = next((k for alias_kana in kanas if alias_kana for k in in_snapshot
                         if normalise_key(k) == normalise_key(alias_kana)), None)
            rule = 2
            if kana is None and len(in_snapshot) == 1:
                kana, rule = in_snapshot[0], 3
            if kana is None:
                kana, rule = next((k for k in kanas if k), None), 4
                if in_snapshot:
                    review["authored-unmerged"].append(f"{','.join(aliases)}|{surface}|{'/'.join(in_snapshot)}")
        counts[f"authored_rule_{rule}"] += 1
        if kana is None:
            counts["authored_without_kana"] += 1
            kana = ""
        if kana:
            add_reviewed(kana, surface, weight)
        elif not any(words[i]["surface"] == surface and words[i]["reviewed"] for i in by_surface[surface]):
            # No reading at all: a word that is reached only through its authored keys.
            by_surface[surface].append(len(words))
            words.append({"kana": "", "surface": surface, "romaji": "", "pinyin": "", "weight": weight,
                          "class": "n", "tier": "R", "rule": "", "reviewed": True, "no_pinyin": False})

    postings = []

    def post(key, word, weight, kind, flags):
        postings.append({"key": key, "word": word, "weight": max(1, min(10000, weight)), "kind": kind, "flags": flags})

    # READING and VARIANT postings; the pinyin bit is decided per key below.
    candidates = collections.defaultdict(list)  # key -> [(posting, passes)]
    for index, word in enumerate(words):
        if not word["kana"]:
            continue
        key = normalise_key(word["kana"])
        flags = FLAG_REVIEWED if word["reviewed"] else 0
        derived = not word["reviewed"] and not word["no_pinyin"]
        h = letters(key)
        post(key, index, word["weight"], KIND_READING, flags)
        candidates[key].append((postings[-1], derived and passes_reading(h, word["weight"], word["surface"], side, old_forms), h))
        spelled = kana_to_hepburn(word["kana"])
        if spelled and "'" in spelled:
            variant_kana = romaji_to_kana(spelled.replace("'", ""))
            variant_key = normalise_key(variant_kana) if variant_kana else None
            if variant_key and variant_key != key:
                post(variant_key, index, word["weight"] - VARIANT_DEDUCTION, KIND_VARIANT, flags)
                vh = letters(variant_key)
                candidates[variant_key].append((postings[-1], derived and passes_reading(vh, word["weight"], word["surface"], side, old_forms, variant=True), vh))
                counts["variant_postings"] += 1

    # ALIAS postings for derived words; a posting that fails the gate is not written.
    polyphones = set()
    report = json.loads((source / "build-report.json").read_text(encoding="utf-8"))
    for item in report.get("review", {}).get("polyphones", []):
        if item.get("words", 0) >= 1:
            polyphones.add(item["kanji"])
    alias_candidates = collections.defaultdict(list)
    for index, word in enumerate(words):
        if word["reviewed"] or word["no_pinyin"] or not word["pinyin"]:
            continue
        keys = alias_keys(word["surface"], word["pinyin"], word["rule"] or "char", table)
        if not keys:
            continue
        kind = KIND_ALIAS_KANJI if all(is_kanji(ch) for ch in word["surface"]) else KIND_ALIAS_KANA
        missing = sorted({ch for ch in word["surface"] if ch in polyphones and ch not in table})
        for key in keys[:2]:
            if not passes_alias(key, kind, word["weight"], word["surface"], side):
                counts[f"alias_failed_kind{kind}"] += 1
                continue
            deduction = ALIAS_KANJI_DEDUCTION if kind == KIND_ALIAS_KANJI else ALIAS_KANA_DEDUCTION
            alias_candidates[key].append({"key": key, "word": index, "weight": max(1, word["weight"] - deduction),
                                          "kind": kind, "flags": FLAG_PINYIN_OK})
            if missing and word["rule"] != "word":
                review["alias-polyphones"].append(f"{key}\t{word['surface']}\t{word['kana']}\t{''.join(missing)}")

    # AUTHORED postings: today's native-japanese.json, key for key, entry for entry.
    authored_keys = set(japanese)
    for reading, entries in japanese.items():
        for entry in entries:
            choices = [i for i in by_surface[entry["text"]] if words[i]["reviewed"]]
            if not choices:
                raise ValueError(f"authored entry without a reviewed word: {reading}|{entry['text']}")
            target = romaji_to_kana(reading)
            target = normalise_key(target) if target else None
            word = next((i for i in choices if target and normalise_key(words[i]["kana"]) == target), choices[0])
            post(reading, word, entry["weight"], KIND_AUTHORED, FLAG_REVIEWED | FLAG_PINYIN_OK)

    # Slot limit per key: the best passing derived posting and at most SLOT_EXTRA more.
    collisions = []
    for key, group in candidates.items():
        given = 0
        for posting, passes, h in sorted(group, key=lambda item: posting_order(item[0], words)):
            if passes and given <= SLOT_EXTRA:
                posting["flags"] |= FLAG_PINYIN_OK
                given += 1
                if posting["kind"] == KIND_READING and side.zc.get(h, 0) > 0:
                    word = words[posting["word"]]
                    collisions.append(f"{h}\t{side.best[h]}\t{side.zc[h]}\t{word['surface']}\t{word['kana']}\t{word['weight']}")
            elif passes:
                counts["slot_limit_dropped"] += 1
    authored_surfaces = collections.defaultdict(set)
    for posting in postings:
        if posting["key"] in authored_keys:
            authored_surfaces[posting["key"]].add(words[posting["word"]]["surface"])
    for key, group in alias_candidates.items():
        group.sort(key=lambda posting: posting_order(posting, words))
        seen = set(authored_surfaces.get(key, ()))
        kept = 0
        for posting in group:
            surface = words[posting["word"]]["surface"]
            if surface in seen or kept > SLOT_EXTRA:
                counts["alias_dropped_in_key"] += 1
                continue
            seen.add(surface)
            postings.append(posting)
            kept += 1
            counts[f"alias_kind{posting['kind']}"] += 1
            if posting["kind"] == KIND_ALIAS_KANJI and side.zc.get(key, 0) > 0:
                counts["alias_kind3_on_chinese_reading"] += 1

    # One surface once per key: keep the first in stored order.
    by_key = collections.defaultdict(list)
    for posting in postings:
        by_key[posting["key"]].append(posting)
    final = []
    for key, group in by_key.items():
        group.sort(key=lambda posting: posting_order(posting, words))
        seen = set()
        for posting in group:
            surface = words[posting["word"]]["surface"]
            if surface in seen:
                counts["same_surface_dropped"] += 1
                continue
            seen.add(surface)
            final.append(posting)
        if len(seen) > 64:
            raise ValueError(f"more than 64 postings under {key}")

    # Reviewed section: one row per key of today's native-japanese.json.
    reviewed_rows = []
    for reading, entries in japanese.items():
        first = entries[0]
        pool = core_eligible(reading, first["weight"])
        reviewed_rows.append({"reading": reading, "text": first["text"], "weight": first["weight"],
                              "flags": REVIEWED_POOL if pool else 0})
    pool_count = sum(1 for row in reviewed_rows if row["flags"] & REVIEWED_POOL)
    if pool_count > MAX_POOL or len(reviewed_rows) > 4096:
        raise ValueError(f"reviewed section: {pool_count} pool rows, {len(reviewed_rows)} rows")
    counts["pool_rows"] = pool_count
    review["collisions"] = sorted(collisions)
    image_words = [{"surface": w["surface"], "class": CLASSES.index(w["class"]),
                    "flags": FLAG_REVIEWED if w["reviewed"] else 0} for w in words]
    return image_words, final, reviewed_rows, review, counts


def core_eligible(reading, weight):
    """The rule of Session::add_japanese_word (crates/meowpuut-core/src/session.rs)."""
    return (reading.isascii() and reading.isalpha() and len(reading) <= 20
            and (len(reading) >= 3 or reading == "au" and weight >= 4000)
            and (weight >= 4000 or weight == 3500))


def pinyin_bits(postings):
    return {(p["key"], p["word"], p["kind"]): bool(p["flags"] & FLAG_PINYIN_OK) for p in postings}


def entries_only_rows(pinyin_rows, root=ROOT):
    """Rows of the merged pinyin table whose Simplified text is an entry of mimo-tc or mimo-sc.

    Used only for the report of design question 9 (how many gate bits change when the gate
    ignores words that come only from the phrases or the project overlays).
    """
    from mimo_pipeline import load_mimo_sc_dataset, load_mimo_tc_dataset
    texts = {hans for _hant, hans, _code, _w in load_mimo_tc_dataset()["entries"]}
    texts |= {hans for hans, _reading, _f in load_mimo_sc_dataset()["entries"]}
    return [row for row in pinyin_rows if row[0][3] in texts]


def write_mimo_jp(output, pinyin_rows, japanese, review_dir, root=ROOT):
    """Write <output>/mimo_jp.bin and the review lists; return counts for the merge report."""
    output, review_dir = pathlib.Path(output), pathlib.Path(review_dir)
    words, postings, reviewed, review, counts = build(root, pinyin_rows, japanese)
    base = output / "mimo_base.bin"
    chinese_build = chinese_build_of(base.read_bytes()) if base.exists() else 0
    data = write_image(words, postings, reviewed, chinese_build)
    (output / "mimo_jp.bin").write_bytes(data)
    # The Python reader must give back what was written.
    back = read_image(data)
    expected = sorted((p["key"], p["word"], p["weight"], p["kind"], p["flags"]) for p in postings)
    got = sorted((p["key"], p["word"], p["weight"], p["kind"], p["flags"]) for p in back["postings"])
    if got != expected or back["words"] != words or len(back["reviewed"]) != len(reviewed):
        raise ValueError("mimo_jp.bin does not read back as written")
    # Design question 9: gate bits when only mimo-tc and mimo-sc entries count as Chinese.
    alt = build(root, entries_only_rows(pinyin_rows, root), japanese)[1]
    main_bits, alt_bits = pinyin_bits(postings), pinyin_bits(alt)
    changed = sum(1 for key in main_bits.keys() | alt_bits.keys() if main_bits.get(key, False) != alt_bits.get(key, False))
    review_dir.mkdir(parents=True, exist_ok=True)
    headers = {
        "collisions": "# reading\tbest Chinese word\tits weight\tJapanese word\tkana\tits weight",
        "authored-unmerged": "# aliases|surface|kana of the snapshot rows with this surface",
        "alias-polyphones": "# alias key\tsurface\tkana\tpolyphonic kanji without a line in japanese-kanji-pinyin.tsv",
        "deny-unmatched": "# rows of japanese-deny.tsv that match no snapshot row",
    }
    for name, header in headers.items():
        lines = review.get(name, [])
        (review_dir / f"{name}.tsv").write_text(header + "\n" + "".join(line + "\n" for line in lines), encoding="utf-8")
    stats = {
        "japanese_image": "mimo_jp.bin",
        "japanese_image_bytes": len(data),
        "japanese_image_sha256": hashlib.sha256(data).hexdigest(),
        "japanese_words": len(words),
        "japanese_keys": len({p["key"] for p in postings}),
        "japanese_postings": len(postings),
        "japanese_reviewed_rows": len(reviewed),
        "japanese_pinyin_ok_postings": sum(1 for p in postings if p["flags"] & FLAG_PINYIN_OK),
        "japanese_collisions": len(review.get("collisions", [])),
        "japanese_gate_bits_changed_entries_only": changed,
        "japanese_counts": dict(sorted(counts.items())),
    }
    print("mimo-jp image:", json.dumps({k: v for k, v in stats.items() if k != "japanese_counts"}, ensure_ascii=False))
    return stats
