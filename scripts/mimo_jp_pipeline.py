#!/usr/bin/env python3
"""mimo-jp data pipeline: fetch the pinned inputs, build the snapshot, verify it.

  mimo_jp_pipeline.py fetch  [--cache DIR] [--seed DIR] [--jobs N]
  mimo_jp_pipeline.py build  [--cache DIR] [--out DIR] [--cost-cut N] [--dump-r6 FILE]
  mimo_jp_pipeline.py verify [--cache DIR]

fetch   checks every input against the checksum table below. A missing file is copied
        from --seed (a directory with the same layout) or downloaded.
build   writes mimo-jp.tsv, kanji-pinyin.tsv, build-report.json and SOURCE.json, plus the
        two new notice files, into --out. It never writes into DataSources/.
verify  builds into a temporary directory and compares with DataSources/mimo-jp.

Python standard library only. Normal app builds do not run this script.
"""
import argparse
import collections
import concurrent.futures
import csv
import functools
import hashlib
import itertools
import json
import math
import pathlib
import re
import shutil
import struct
import sys
import tempfile
import time
import unicodedata
import urllib.request
import zipfile
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_CACHE = ROOT / ".build/mimo-jp-cache"
DEFAULT_OUT = ROOT / ".build/mimo-jp-snapshot"
SNAPSHOT_DIR = ROOT / "DataSources/mimo-jp"
VERSION = "2.0"
GENERATED = ("mimo-jp.tsv", "kanji-pinyin.tsv", "build-report.json", "SOURCE.json")

# --------------------------------------------------------------------------- inputs

MOZC_REV = "a4541df29db443b49f81cf54665873b68fde9efa"
MOZC_RAW = f"https://raw.githubusercontent.com/google/mozc/{MOZC_REV}/"
MOZC_DIR = "src/data/dictionary_oss/"
OPENCC_REV = "fae2a0a911c30764e669a7d8b0591cd8e852934a"
OPENCC_RAW = f"https://raw.githubusercontent.com/BYVoid/OpenCC/{OPENCC_REV}/"
UNIDIC_URL = "https://clrd.ninjal.ac.jp/unidic_archive/2512/unidic-cwj-202512_full.zip"
UNIDIC_ARCHIVE_SHA256 = "ca02ad51c8ae238373dcb150eec014f1ad9f05f54c7416a16c2fe444fd4921b3"
UNIHAN_URL = "https://www.unicode.org/Public/18.0.0/ucd/Unihan.zip"
UNICODE_LICENSE_URL = "https://www.unicode.org/license.txt"

# cache path, sha256, bytes, how to get it:
#   ("url", URL)                  plain download
#   ("remote-zip", URL, MEMBER)   one member of a remote zip, read with range requests
#   ("local-zip", PATH, MEMBER)   one member of a zip that is itself in the cache
INPUTS = [
    ("mozc/dictionary00.txt", "9e07ce292932c4fff327ed8cb06274c09acca556bd8bdf7c7ef1f6173eae3b61", 7157174),
    ("mozc/dictionary01.txt", "836fd5fd8399510de0d8ef7b33be913cca6bbaee7b712cc8045fba03f4c75a89", 6428663),
    ("mozc/dictionary02.txt", "93943858d0cfd092b849a78def6901eaf228177e1ac5663544e865a3a1e45a9e", 6480134),
    ("mozc/dictionary03.txt", "cb9f1b0b4a7550d682d86244a021cd17e0b15366328819e69135b5f183a1a24b", 7434257),
    ("mozc/dictionary04.txt", "9fdc4e3948d03463de13ab5782adffbc11dd06e934cb80138c5920029cd3d2f6", 5399172),
    ("mozc/dictionary05.txt", "199e3b0f050b691823ae7ba943090d437f51af6436b27c5f3dc69cafa12000e4", 5316454),
    ("mozc/dictionary06.txt", "b95b14f7770ae2a824bc8e9fad8b03c1bc45c504f93a36554b8b56daa0368d2a", 5373350),
    ("mozc/dictionary07.txt", "7f53649b529d4c8c495d3a8dd9c4d489dfe7696080f7423107ff01331174be48", 5315756),
    ("mozc/dictionary08.txt", "50a31bb5fc1646721b00be371a5695dffab1292bda143e1e59cc70cbbdd8643f", 4887114),
    ("mozc/dictionary09.txt", "4e5fcd4cbfbf11395166a37b506f547fc54d20edc0884f5184ecd3fc705f0d9a", 6690572),
    ("mozc/connection_single_column.txt", "68823a1f6617372ccb89afdbcb334d7a0bcfa82c9c5dda03fc1f80f1cab5c48e", 36428642),
    ("mozc/id.def", "07a05a268c3783b7e02b36e4aa591555a2dd19a1f3c93891576740761c154f2d", 151747),
    ("mozc/reading_correction.tsv", "08066c2af7e6d3eba0ff7b1a9e10d876436b639d036680d1f7f91a8306ec22d7", 29590),
    ("mozc/LICENSE", "44cdd923b91ea9199293abecc2762c70c87dbf1e581c027a94c416368d1a648c", 5734,
     ("url", MOZC_RAW + "LICENSE")),
    ("unidic-cwj-202512/lex.csv", "bd00a695ba897a3250965257341e1929ae062dfa2e63b223a05bcc02f22e74a2", 275464163,
     ("remote-zip", UNIDIC_URL, "lex.csv")),
    ("unidic-cwj-202512/license/BSD", "9980b1824f0d1dac41ea93dc96780c222e5cca3c4d1b5677ac8ab923a8533c3f", 1523,
     ("remote-zip", UNIDIC_URL, "license/BSD")),
    ("unidic-cwj-202512/license/COPYING", "2694852d16fccf37701a2fb85435a860bd0be9d697e8ff2e35b7d35b50129fe0", 230,
     ("remote-zip", UNIDIC_URL, "license/COPYING")),
    ("opencc/JPShinjitaiCharacters.txt", "12cec7250b873ef52b36d8f92218d4f92c0aaf5d8cd7c58fe42d9785bdcdc43a", 12442,
     ("url", OPENCC_RAW + "data/dictionary/JPShinjitaiCharacters.txt")),
    ("opencc/LICENSE", "b534e465949558eec2597b04f5092b5e161236a68dfbfd04d547592ac3964308", 9165,
     ("url", OPENCC_RAW + "LICENSE")),
    ("unihan/Unihan.zip", "4c93ea9c1f636451729a840978f1667a53886af37ba854fdcce109721c63d43e", 8340649,
     ("url", UNIHAN_URL)),
    ("unihan/Unihan_Readings.txt", "9d39995b5de714e8ce93716ed5d15eaa0792d68e407cdf8a0add2893b8f4150b", 8844571,
     ("local-zip", "unihan/Unihan.zip", "Unihan_Readings.txt")),
    ("unihan/Unihan_Variants.txt", "4ccfd34b08ac7b9c14353f6668ba43e3796d85c4cdce7952b635be16015aafa2", 726700,
     ("local-zip", "unihan/Unihan.zip", "Unihan_Variants.txt")),
    ("unihan/license.txt", "e7a93b009565cfce55919a381437ac4db883e9da2126fa28b91d12732bc53d96", 1995,
     ("url", UNICODE_LICENSE_URL)),
]
INPUTS = [row if len(row) == 4 else row + (("url", MOZC_RAW + MOZC_DIR + row[0].split("/", 1)[1]),) for row in INPUTS]
INPUT_SHA = {path: sha for path, sha, _, _ in INPUTS}

# Notice files that already sit in DataSources/mimo-jp. LICENSE.txt is pinned here so
# that SOURCE.json stays a function of pinned data.
EXISTING_LICENSE_SHA256 = "770a75de30705439084f869dbcb0bc4ebcffcb7c7124c0d74f5083170318a9bb"

# --------------------------------------------------------------------------- parameters

COST_CUT = 7800          # R6, adjusted cost, tier A
BC_GAP = 400             # tiers B and C are cut this much lower
PLACE_CUT = 6600
STEM_PENALTY = 800
# R5. Median gap between the Mozc dictionary-form cost and the TUBELEX-JA lemma cost
# (naist-nlp/tubelex 7cb5fb36, BSD-3-Clause), measured once. TUBELEX is not read here.
CLASS_OFFSETS = {"verb": 680, "adj": 340, "adverb": 200, "interjection": 600, "pronoun": 390}
WEIGHT_TEXT = "round(700*ln(1+59.7e6*exp(-cost/500))) clamped to 1..10000"
ROUNDING_GUARD = 1e-6

CLASS_NAMES = {"noun": "n", "pronoun": "pn", "adj": "adj-i", "na-adj": "adj-na", "adverb": "adv",
               "adnominal": "adn", "conjunction": "conj", "interjection": "int", "place": "place"}
GODAN_ROWS = {"カ行": "v5k", "ガ行": "v5g", "サ行": "v5s", "タ行": "v5t", "ナ行": "v5n", "バ行": "v5b",
              "マ行": "v5m", "ラ行": "v5r", "ワア行": "v5w"}

# --------------------------------------------------------------------------- small helpers

HIRA_KEY = re.compile(r"[ぁ-ゖー]+\Z")
SURFACE_OK = re.compile(r"[ぁ-ゖァ-ヺー一-鿿㐀-䶿々〆ヶ]+\Z")
KANJI = re.compile(r"[一-鿿㐀-䶿]")
KANA = re.compile(r"[ぁ-ゖァ-ヺー]")
KATA_ONLY = re.compile(r"[ァ-ヺー]+\Z")
HIRA_ONLY = re.compile(r"[ぁ-ゖー]+\Z")
REPEAT_MARK = "々"
CLOSING_MARK = "〆"   # U+3006


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def kata_to_hira(text):
    return "".join(chr(ord(c) - 0x60) if "ァ" <= c <= "ヶ" else c for c in text)


def say(*parts):
    print(*parts, flush=True)


# --------------------------------------------------------------------------- fetch

USER_AGENT = "Mozilla/5.0 (mimo-jp-pipeline)"
CHUNK = 1 << 20


def _request(url, start=None, end=None, method="GET", tries=8):
    headers = {"User-Agent": USER_AGENT}
    if start is not None:
        headers["Range"] = f"bytes={start}-{end}"
    error = None
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers=headers, method=method)
            with urllib.request.urlopen(req, timeout=120) as response:
                if method == "HEAD":
                    return int(response.headers["Content-Length"])
                data = response.read()
            if start is not None and len(data) != end - start + 1:
                raise OSError(f"short range reply: {len(data)} bytes")
            return data
        except (OSError, ValueError, TypeError) as exc:
            error = exc
            time.sleep(1 + attempt)
    raise OSError(f"{url}: {error}")


def _ranges(start, size):
    return [(start + offset, start + min(size, offset + CHUNK) - 1) for offset in range(0, size, CHUNK)]


def download(url, dest, size, jobs):
    """Download `size` bytes with parallel range requests, written in order."""
    with open(dest, "wb") as out, concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        for data in pool.map(lambda r: _request(url, r[0], r[1]), _ranges(0, size)):
            out.write(data)


def remote_zip_entries(url):
    """Central directory of a remote zip: member name -> (method, crc, compressed size, header offset)."""
    size = _request(url, method="HEAD")
    tail_len = min(size, 256 * 1024)
    tail = _request(url, size - tail_len, size - 1)
    pos = tail.rfind(b"PK\x05\x06")
    if pos < 0:
        raise OSError(f"{url}: no end of central directory")
    _, _, _, _, total, cd_size, cd_offset, _ = struct.unpack("<IHHHHIIH", tail[pos:pos + 22])
    locator = tail.rfind(b"PK\x06\x07", 0, pos)
    if locator >= 0 and (cd_offset == 0xFFFFFFFF or total == 0xFFFF or cd_size == 0xFFFFFFFF):
        _, _, record_offset, _ = struct.unpack("<IIQI", tail[locator:locator + 20])
        record = _request(url, record_offset, record_offset + 55)
        cd_size, cd_offset = struct.unpack("<IQHHIIQQQQ", record)[8:10]
    cd = _request(url, cd_offset, cd_offset + cd_size - 1)
    entries, p = {}, 0
    while p < len(cd) and cd[p:p + 4] == b"PK\x01\x02":
        (_, _, _, flags, method, _, _, crc, csize, usize, nlen, elen, clen, _, _, _, offset) = struct.unpack(
            "<IHHHHHHIIIHHHHHII", cd[p:p + 46])
        name = cd[p + 46:p + 46 + nlen].decode("utf-8" if flags & 0x800 else "cp437", errors="replace")
        extra, q = cd[p + 46 + nlen:p + 46 + nlen + elen], 0
        while q + 4 <= len(extra):
            tag, length = struct.unpack("<HH", extra[q:q + 4])
            body, b = extra[q + 4:q + 4 + length], 0
            if tag == 1:   # zip64 sizes, in this order, only for fields that are 0xFFFFFFFF
                if usize == 0xFFFFFFFF: usize = struct.unpack("<Q", body[b:b + 8])[0]; b += 8
                if csize == 0xFFFFFFFF: csize = struct.unpack("<Q", body[b:b + 8])[0]; b += 8
                if offset == 0xFFFFFFFF: offset = struct.unpack("<Q", body[b:b + 8])[0]; b += 8
            q += 4 + length
        entries[name] = (method, crc, csize, offset)
        p += 46 + nlen + elen + clen
    return entries


def fetch_remote_zip_member(url, entry, dest, jobs):
    method, crc_expected, csize, offset = entry
    if method not in (0, 8):
        raise OSError(f"{url}: unsupported zip method {method}")
    header = _request(url, offset, offset + 29)
    nlen, elen = struct.unpack("<HH", header[26:30])
    inflater = zlib.decompressobj(-15) if method == 8 else None
    crc = 0
    with open(dest, "wb") as out, concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        for data in pool.map(lambda r: _request(url, r[0], r[1]), _ranges(offset + 30 + nlen + elen, csize)):
            plain = inflater.decompress(data) if inflater else data
            crc = zlib.crc32(plain, crc)
            out.write(plain)
        if inflater:
            plain = inflater.flush()
            crc = zlib.crc32(plain, crc)
            out.write(plain)
    if crc != crc_expected:
        raise OSError(f"{url}: crc mismatch")


def input_state(cache, path, sha, size):
    target = cache / path
    if not target.is_file():
        return "missing"
    if target.stat().st_size != size or sha256_file(target) != sha:
        return "wrong checksum"
    return "ok"


def cmd_fetch(args):
    cache = pathlib.Path(args.cache)
    seed = pathlib.Path(args.seed) if args.seed else None
    zip_entries, failed = {}, []
    for path, sha, size, how in INPUTS:
        target = cache / path
        state = input_state(cache, path, sha, size)
        note = "present"
        if state == "wrong checksum":
            # Never replaced silently: a changed input needs a person to look at it.
            say(f"FAIL     {path}: checksum or size differs from the table; delete the file to fetch it again")
            failed.append(path)
            continue
        if state == "missing":
            target.parent.mkdir(parents=True, exist_ok=True)
            partial = target.with_name(target.name + ".part")
            try:
                if seed and (seed / path).is_file():
                    shutil.copyfile(seed / path, partial)
                    note = "copied from seed"
                elif how[0] == "url":
                    download(how[1], partial, size, args.jobs)
                    note = "downloaded"
                elif how[0] == "remote-zip":
                    if how[1] not in zip_entries:
                        zip_entries[how[1]] = remote_zip_entries(how[1])
                    fetch_remote_zip_member(how[1], zip_entries[how[1]][how[2]], partial, args.jobs)
                    note = "downloaded (zip member)"
                else:
                    with zipfile.ZipFile(cache / how[1]) as archive, open(partial, "wb") as out:
                        shutil.copyfileobj(archive.open(how[2]), out)
                    note = "unpacked"
            except (OSError, KeyError, zipfile.BadZipFile) as exc:
                say(f"FAIL     {path}: {exc}")
                partial.unlink(missing_ok=True)
                failed.append(path)
                continue
            if partial.stat().st_size != size or sha256_file(partial) != sha:
                say(f"FAIL     {path}: fetched file does not match the table (left as {partial.name})")
                failed.append(path)
                continue
            partial.replace(target)
        say(f"verified {path} ({note})")
    say(f"{len(INPUTS) - len(failed)} of {len(INPUTS)} inputs present and verified in {cache}")
    return 1 if failed else 0


def require_inputs(cache):
    bad = [f"{path}: {state}" for path, sha, size, _ in INPUTS
           for state in [input_state(cache, path, sha, size)] if state != "ok"]
    if bad:
        raise SystemExit("inputs are not the pinned ones, run fetch first:\n  " + "\n  ".join(bad))


# --------------------------------------------------------------------------- romaji

def _romaji_table():
    """The table of Sources/JapaneseBackend.swift plus dhu and thu. Returns (all tokens, strict tokens)."""
    table = {}
    rows = ["a i u e o|あいうえお", "ka ki ku ke ko|かきくけこ", "sa shi su se so|さしすせそ",
            "ta chi tsu te to|たちつてと", "na ni nu ne no|なにぬねの", "ha hi fu he ho|はひふへほ",
            "ma mi mu me mo|まみむめも", "ya yu yo|やゆよ", "ra ri ru re ro|らりるれろ", "wa wo|わを",
            "ga gi gu ge go|がぎぐげご", "za ji zu ze zo|ざじずぜぞ", "da di du de do|だぢづでど",
            "ba bi bu be bo|ばびぶべぼ", "pa pi pu pe po|ぱぴぷぺぽ", "va vi vu ve vo|ゔぁ,ゔぃ,ゔ,ゔぇ,ゔぉ",
            "xa xi xu xe xo|ぁぃぅぇぉ", "xya xyu xyo|ゃゅょ", "xtsu|っ", "xwa|ゎ"]
    for row in rows:
        keys, values = row.split("|")
        values = values.split(",") if "," in values else list(values)
        table.update(zip(keys.split(), values))
    for prefix, base in (("ky", "き"), ("gy", "ぎ"), ("sh", "し"), ("ch", "ち"), ("ny", "に"), ("hy", "ひ"),
                         ("by", "び"), ("py", "ぴ"), ("my", "み"), ("ry", "り"), ("j", "じ")):
        for vowel, small in (("a", "ゃ"), ("u", "ゅ"), ("o", "ょ"), ("e", "ぇ")):
            table[prefix + vowel] = base + small
    kunrei = {"si": "shi", "ti": "chi", "tu": "tsu", "hu": "fu", "zi": "ji", "sya": "sha", "syu": "shu",
              "syo": "sho", "tya": "cha", "tyu": "chu", "tyo": "cho", "cya": "cha", "cyu": "chu", "cyo": "cho",
              "zya": "ja", "zyu": "ju", "zyo": "jo", "jya": "ja", "jyu": "ju", "jyo": "jo", "xtu": "xtsu"}
    for alias, standard in kunrei.items():
        table[alias] = table[standard]
    for key in [k for k in table if k.startswith("x")]:
        table["l" + key[1:]] = table[key]
    table.update({"fa": "ふぁ", "fi": "ふぃ", "fe": "ふぇ", "fo": "ふぉ", "wi": "うぃ", "we": "うぇ", "who": "うぉ",
                  "she": "しぇ", "che": "ちぇ", "thi": "てぃ", "dhi": "でぃ", "twu": "とぅ", "dwu": "どぅ",
                  "fyu": "ふゅ", "ye": "いぇ", "dhu": "でゅ", "thu": "てゅ"})
    strict = {k: v for k, v in table.items() if k not in kunrei and k[0] not in "xl"}
    return table, strict


ROMAJI_TABLE, _STRICT = _romaji_table()
KANA_TO_HEPBURN = {}
for _token in sorted(_STRICT):
    if _STRICT[_token] in KANA_TO_HEPBURN:
        raise AssertionError(f"two Hepburn tokens for {_STRICT[_token]}")
    KANA_TO_HEPBURN[_STRICT[_token]] = _token

_VOWEL_KANA = {}
for _row, _vowel in (("あかさたなはまやらわがざだばぱぁゃゎ", "あ"), ("いきしちにひみりぎじぢびぴぃ", "い"),
                     ("うくすつぬふむゆるぐずづぶぷぅゅゔ", "う"), ("えけせてねへめれげぜでべぺぇ", "え"),
                     ("おこそとのほもよろをごぞどぼぽぉょ", "お")):
    for _ch in _row:
        _VOWEL_KANA[_ch] = _vowel


def normalise_key(kana):
    """Katakana to hiragana; ー becomes the vowel kana of the kana before it (design 7.2)."""
    out = []
    for ch in kata_to_hira(kana):
        out.append(_VOWEL_KANA.get(out[-1], ch) if ch == "ー" and out else ch)
    return "".join(out)


def romaji_to_kana(raw):
    """Port of Romaji.reading in Sources/JapaneseBackend.swift, used here only to check column 3."""
    text, out, i = raw.lower(), [], 0
    while i < len(text):
        key = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else None
        if key == "-":
            out.append("ー"); i += 1; continue
        if key == "'":
            i += 1; continue
        if key == "n":
            if nxt == "'":
                out.append("ん"); i += 2; continue
            if nxt is None:
                out.append("ん"); i += 1; continue
            if nxt == "n":
                out.append("ん"); i += 2 if i + 2 == len(text) else 1; continue
            if nxt not in "aiueoy":
                out.append("ん"); i += 1; continue
        if key == nxt and key not in "aiueon":
            out.append("っ"); i += 1; continue
        for count in range(min(4, len(text) - i), 0, -1):
            value = ROMAJI_TABLE.get(text[i:i + count])
            if value is not None:
                out.append(value); i += count
                break
        else:
            out.append(key); i += 1
    return "".join(out)


def _token_at(kana, i):
    return KANA_TO_HEPBURN.get(kana[i:i + 2]) if len(kana[i:i + 2]) == 2 and kana[i:i + 2] in KANA_TO_HEPBURN \
        else KANA_TO_HEPBURN.get(kana[i:i + 1])


def kana_to_hepburn(kana):
    """Hepburn spelling that romaji_to_kana turns back into `kana`; "" when there is none.

    ん is n, or n' before a vowel or y. っ doubles the next consonant. ー doubles the vowel.
    A reading that needs an x-kana (small kana on its own, っ at the end or before a vowel or n)
    gets "".
    """
    out, i = [], 0
    while i < len(kana):
        ch = kana[i]
        if ch == "ー":
            if not out or out[-1][-1] not in "aiueo":
                return ""
            out.append(out[-1][-1]); i += 1
        elif ch == "っ":
            head = _token_at(kana, i + 1)
            if not head or head[0] in "aiueon":
                return ""
            out.append(head[0]); i += 1
        elif ch == "ん":
            head = _token_at(kana, i + 1)
            out.append("n'" if head and head[0] in "aiueoy" else "n"); i += 1
        elif kana[i:i + 2] in KANA_TO_HEPBURN and len(kana[i:i + 2]) == 2:
            out.append(KANA_TO_HEPBURN[kana[i:i + 2]]); i += 2
        elif ch in KANA_TO_HEPBURN:
            out.append(KANA_TO_HEPBURN[ch]); i += 1
        else:
            return ""
    romaji = "".join(out)
    return romaji if normalise_key(romaji_to_kana(romaji)) == normalise_key(kana) else ""


# --------------------------------------------------------------------------- Mozc and UniDic

def load_ids(cache):
    ids = {}
    for line in (cache / "mozc/id.def").read_text(encoding="utf-8").splitlines():
        number, name = line.split(" ", 1)
        ids[int(number)] = name.split(",")
    return ids


def load_connection(cache):
    """(BOS -> left id, right id -> EOS). Line rid * N + lid after the size line; BOS and EOS are id 0."""
    with (cache / "mozc/connection_single_column.txt").open(encoding="ascii") as f:
        size = int(f.readline())
        bos, eos = [0] * size, [0] * size
        for index, line in enumerate(f):
            rid, lid = divmod(index, size)
            if rid == 0:
                bos[lid] = int(line)
            if lid == 0:
                eos[rid] = int(line)
    bos[0] = eos[0] = 0
    return bos, eos


def mozc_rows(cache):
    for i in range(10):
        with (cache / f"mozc/dictionary{i:02d}.txt").open(encoding="utf-8") as f:
            for line in f:
                parts = line.rstrip("\n").split("\t")
                yield parts[0], int(parts[1]), int(parts[2]), int(parts[3]), parts[4], parts[5:]


def pos_class(feature):
    """Collapse a Mozc POS feature into the classes the selection uses."""
    p1, p2, p3, _, _, cform, _ = (feature + ["*"] * 7)[:7]
    if p1 == "名詞":
        if p2 == "固有名詞":
            return {"人名": "name", "地域": "place", "組織": "org"}.get(p3, "proper")
        if p2 in ("一般", "サ変接続", "形容動詞語幹", "副詞可能", "ナイ形容詞語幹"):
            return "noun"
        if p2 == "代名詞":
            return "pronoun"
        if p2 == "数":
            return "number"
        return "noun-bound"
    if p1 == "動詞":
        if p2 != "自立":
            return "verb-bound"
        return "verb" if cform == "基本形" else "verb-form"
    if p1 == "形容詞":
        if p2 != "自立":
            return "adj-bound"
        return "adj" if cform == "基本形" else "adj-form"
    if p1 == "副詞":
        return "adverb"
    if p1 in ("連体詞", "接続詞", "感動詞"):
        return {"連体詞": "adnominal", "接続詞": "conjunction", "感動詞": "interjection"}[p1]
    if p1 == "接頭詞":
        return "prefix"
    return "function"


def surface_kind(surface):
    if KANJI.search(surface):
        return "mixed" if KANA.search(surface) else "kanji"
    if KATA_ONLY.match(surface):
        return "kata"
    if HIRA_ONLY.match(surface):
        return "hira"
    return "kana-mixed"


INF = 10 ** 9
UNIDIC_CLASS = {"動詞": "verb", "形容詞": "adj", "形状詞": "na-adj", "副詞": "adverb", "代名詞": "pronoun",
                "連体詞": "adnominal", "接続詞": "conjunction", "感動詞": "interjection", "接尾辞": "suffix",
                "接頭辞": "prefix", "助詞": "particle", "助動詞": "aux"}
CONTENT = {"noun", "verb", "adj", "na-adj", "adverb", "pronoun", "adnominal", "conjunction", "interjection"}
CLASS_ORDER = ["noun", "verb", "adj", "na-adj", "adverb", "pronoun", "adnominal", "conjunction", "interjection"]


def unidic_class(row):
    p1, p2, p3 = row[4], row[5], row[6]
    if p1 == "名詞":
        if p2 == "固有名詞":
            return {"人名": "name", "地名": "place"}.get(p3, "proper")
        return "noun" if p2 == "普通名詞" else "number"
    return UNIDIC_CLASS.get(p1, "other")


class UniDic:
    """What the selection needs from lex.csv, read in one pass.

    Fields (from 0): 0 surface, 4-6 part of speech, 8 cType, 9 cForm, 14 orthBase, 16 goshu, 24 kana.
    """

    def __init__(self, cache):
        self.base = collections.defaultdict(set)       # (kana, surface) -> classes of the base form
        self.parts = collections.defaultdict(set)      # surface -> kana of forms that can be a word part
        self.suffix_parts = collections.defaultdict(set)
        self.ctypes = collections.defaultdict(set)     # (kana, surface) -> cType of verb and adjective base forms
        self.loanwords = set()                         # orthBase of non-proper rows with goshu 外
        csv.field_size_limit(1 << 30)
        with (cache / "unidic-cwj-202512/lex.csv").open(encoding="utf-8", newline="") as f:
            for r in csv.reader(f):
                cls = unidic_class(r)
                kana = kata_to_hira(r[24])
                form = r[9]
                if form in ("*", "終止形-一般"):
                    self.base[(kana, r[0])].add(cls)
                    if cls in ("verb", "adj"):
                        self.ctypes[(kana, r[0])].add(r[8])
                if r[16] == "外" and not (r[4] == "名詞" and r[5] == "固有名詞"):
                    self.loanwords.add(r[14])
                if cls in ("particle", "aux", "other", "name", "number"):
                    continue
                if form in ("*", "終止形-一般", "連用形-一般") or (cls == "adj" and form == "語幹-一般"):
                    self.parts[r[0]].add(kana)
                    if cls == "suffix":
                        self.suffix_parts[r[0]].add(kana)

    def split(self, surface, reading, depth=3):
        """Fewest parts whose surfaces and readings concatenate to the pair; 0 if none."""
        parts = self.parts

        @functools.lru_cache(maxsize=None)
        def go(surface, reading, depth):
            if depth == 0:
                return 0
            best = 0
            for cut in range(1, len(surface) + 1):
                for kana in parts.get(surface[:cut], ()):
                    if not reading.startswith(kana):
                        continue
                    if cut == len(surface):
                        if kana == reading:
                            return 1
                        continue
                    rest = go(surface[cut:], reading[len(kana):], depth - 1)
                    if rest and (best == 0 or rest + 1 < best):
                        best = rest + 1
            return best

        self.split = go
        return go(surface, reading, depth)


# --------------------------------------------------------------------------- selection R0-R9

CONTENT_SLOT, STEM, PLACE, NSUFFIX, COMPOUND = 0, 1, 2, 3, 4
MOZC_CONTENT = {"noun", "pronoun", "verb", "adj", "adverb", "adnominal", "conjunction", "interjection"}
COMPOUND_LEFT = {"noun", "verb-form", "adj-form", "prefix", "adverb", "pronoun", "adnominal"}
COMPOUND_RIGHT = {"noun", "verb", "adj", "noun-bound", "verb-form"}

VOICED = {}
for _plain, _voiced in zip("かきくけこさしすせそたちつてとはひふへほ", "がぎぐげござじずぜぞだぢづでどばびぶべぼ"):
    VOICED.setdefault(_voiced, []).append(_plain)
for _plain, _voiced in zip("はひふへほ", "ぱぴぷぺぽ"):
    VOICED.setdefault(_voiced, []).append(_plain)


def select_wide(cache, unidic, stats, cost_cut):
    """R0-R4: (reading, surface, raw cost, tier, class, note) for every pair, before any cost cut."""
    ids = load_ids(cache)
    classes = {number: pos_class(feature) for number, feature in ids.items()}
    bos, eos = load_connection(cache)
    wrong = set()
    for line in (cache / "mozc/reading_correction.tsv").read_text(encoding="utf-8").splitlines():
        if line and not line.startswith("#"):
            word, error = line.split("\t")[:2]
            wrong.add((error, word))

    best = {}   # pair -> [content cost, stem, place, noun+suffix, compound, class of the content row]
    for key, lid, rid, cost, value, extra in mozc_rows(cache):
        stats["mozc rows"] += 1
        if extra or (key, value) in wrong or not HIRA_KEY.match(key) or not SURFACE_OK.match(value):
            stats["R0 dropped"] += 1
            continue
        cls = classes[lid]
        slot = None
        if lid == rid:
            if cls in MOZC_CONTENT:
                slot = CONTENT_SLOT
            elif cls == "verb-form" and ids[lid][5] == "連用形":
                slot = STEM
            elif cls == "place":
                slot = PLACE
        elif cls == "noun" and ids[rid][0] == "名詞" and ids[rid][1] == "接尾":
            slot = NSUFFIX
        elif cls in COMPOUND_LEFT and classes[rid] in COMPOUND_RIGHT:
            slot = COMPOUND   # two POS in one row; used only when UniDic lists the pair as a word
        if slot is None:
            stats["R1/R2 dropped"] += 1
            continue
        item = best.get((key, value))
        if item is None:
            item = best[(key, value)] = [INF, INF, INF, INF, INF, ""]
        total = bos[lid] + cost + eos[rid]
        if total < item[slot]:
            item[slot] = total
            if slot == CONTENT_SLOT:
                item[5] = cls

    rows = []
    for (key, value), item in best.items():
        u = unidic.base.get((key, value), ())
        content_cost, content_cls = item[CONTENT_SLOT], item[5]
        if CONTENT.intersection(u):
            cost, note = content_cost, ""
            if "noun" in u and cost == INF and item[STEM] < INF:
                # No noun row in Mozc: the verb stem row stands in, with a penalty because
                # its cost counts verb use.
                cost, note = item[STEM] + STEM_PENALTY, "stem-as-noun"
            if cost == INF and item[NSUFFIX] < INF:
                cost, note = item[NSUFFIX], "noun+suffix"
            if cost == INF and item[COMPOUND] < INF:
                cost, note = item[COMPOUND], "compound-row"
            if cost < INF:
                rows.append((key, value, cost, "A", next(c for c in CLASS_ORDER if c in u), note))
                stats["A"] += 1
            else:
                stats["UniDic content word, no acceptable Mozc row"] += 1
            continue
        if "place" in u and item[PLACE] <= PLACE_CUT:
            rows.append((key, value, item[PLACE], "A", "place", ""))
            stats["A place"] += 1
            continue
        if u:
            stats["UniDic lists it only as a name, place, affix or number"] += 1
            continue
        # not in UniDic under this reading and spelling
        cost, note = content_cost, ""
        if cost == INF and item[NSUFFIX] < INF:
            cost, content_cls, note = item[NSUFFIX], "noun", "noun+suffix"
        if cost == INF:
            stats["not in UniDic, no content row"] += 1
            continue
        if surface_kind(value) in ("hira", "kata") and content_cls in (
                "adverb", "interjection", "conjunction", "adnominal", "pronoun"):
            if not note:
                rows.append((key, value, cost, "C", content_cls, ""))
                stats["C"] += 1
            else:
                stats["not in UniDic, rejected"] += 1
            continue
        # Rows over the highest cost that R6 can keep are not split; the result is the same.
        if (content_cls in ("noun", "verb", "adj", "interjection") and len(value) >= 3 and len(key) >= 4
                and cost <= cost_cut - BC_GAP + max(CLASS_OFFSETS.values())):
            count = unidic.split(value, key, 3)
            if count >= 2:
                if note == "noun+suffix" and not any(
                        key.endswith(k) for k in unidic.suffix_parts.get(value[-1:], ())):
                    stats["not in UniDic, rejected"] += 1   # the last part must be a UniDic suffix
                    continue
                rows.append((key, value, cost, "B", content_cls, f"parts{count}" + ("," + note if note else "")))
                stats["B"] += 1
                continue
        stats["not in UniDic, rejected"] += 1
    return rows


def cut_rows(wide, cost_cut):
    """R5, R6: (adjusted cost, reading, surface, raw cost, tier, class, note), sorted."""
    rows = []
    for key, value, cost, tier, cls, note in wide:
        adjusted = cost - CLASS_OFFSETS.get(cls, 0)
        if adjusted <= (cost_cut if tier == "A" else cost_cut - BC_GAP):
            rows.append((adjusted, key, value, cost, tier, cls, note))
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    return rows


def voiced_duplicates(rows):
    """R7: pairs whose reading differs from a cheaper row of the same surface only by a voiced first kana."""
    cost = {(r[1], r[2]): r[0] for r in rows}
    return {(key, value) for (key, value), c in cost.items() if key[0] in VOICED and any(
        cost.get((plain + key[1:], value), INF) < c for plain in VOICED[key[0]])}


def kana_duplicates(rows, loanwords):
    """R8: (hiragana pairs to drop, katakana pairs to drop, katakana pairs kept as loanwords)."""
    best = {}
    for adjusted, key, value, *_ in rows:   # rows are sorted by (cost, reading, surface)
        best.setdefault(key, value)
    hira, kata, kept = set(), set(), set()
    for adjusted, key, value, *_ in rows:
        if best[key] == value:
            continue
        if value == key and HIRA_ONLY.match(value):
            hira.add((key, value))
        elif KATA_ONLY.match(value) and kata_to_hira(value) == key:
            (kept if value in loanwords else kata).add((key, value))
    return hira, kata, kept


def conjugation(ctypes):
    """R9: class of a tier A verb from its UniDic cType values.

    v? when every type is classical, and when two modern types compete (する is サ行変格, and
    五段-ラ行 for 擦る). A v? word gets no generated forms; the build report lists these rows.
    """
    found = set()
    for ctype in ctypes:
        if ctype.startswith("五段-"):
            found.add(GODAN_ROWS.get(ctype[3:], "v?"))
        elif ctype.startswith(("下一段", "上一段")):
            found.add("v1")
        elif ctype == "サ行変格":
            found.add("vs")
        elif ctype == "カ行変格":
            found.add("vk")
    found.discard("v?")
    return found.pop() if len(found) == 1 else "v?"


def weight_of(cost):
    """Weight on the scale of the Chinese lexicons (design 6.5), with the rounding guard."""
    value = 700 * math.log(1 + 59.7e6 * math.exp(-cost / 500))
    if abs(value % 1 - 0.5) < ROUNDING_GUARD:
        raise SystemExit(f"weight of cost {cost} is {value!r}, too close to a rounding boundary")
    return max(1, min(10000, round(value)))


def prototype_weight(cost):
    """The research formula. Only --dump-r6 uses it, to compare with the prototype file."""
    w = max(1000, min(6900, round(10063 - 1.0545 * cost)))
    return w + 1 if w == 3500 else w


# --------------------------------------------------------------------------- pinyin of kanji

GUESS_ROUTES = ("old-form+unihan", "unihan-kMandarin", "mimo-sc-rare")


def bare(code):
    return " ".join(re.sub(r"[1-5]$", "", s) for s in code.split())


def strip_tone(marked):
    out = ""
    for ch in unicodedata.normalize("NFD", marked.lower()):
        if ch in "̄́̌̀":
            continue
        if ch == "̈":
            out = out[:-1] + "v"
            continue
        out += ch
    return out


class Pinyin:
    """Mandarin readings for the kanji of Japanese words (design 6.4)."""

    def __init__(self, cache, sc_entries, tc_entries):
        self.tc_char, self.hant_to_hans = collections.defaultdict(dict), {}
        self.tc_word = collections.defaultdict(dict)
        for hant, hans, code, weight in tc_entries:
            if len(hant) == 1:
                self.tc_char[hant][bare(code)] = max(self.tc_char[hant].get(bare(code), 0), weight)
                self.hant_to_hans.setdefault(hant, hans)
            else:
                self.tc_word[hant][bare(code)] = max(self.tc_word[hant].get(bare(code), 0), weight)
        self.sc_char, self.sc_word = {}, collections.defaultdict(dict)
        for hans, code, freq in sc_entries:
            if len(hans) == 1:
                if hans not in self.sc_char or freq > self.sc_char[hans][1]:
                    self.sc_char[hans] = (bare(code), freq)
            else:
                self.sc_word[hans][bare(code)] = max(self.sc_word[hans].get(bare(code), 0), freq)
        self.opencc = {}
        for line in (cache / "opencc/JPShinjitaiCharacters.txt").read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                key, values = line.split("\t", 1)
                self.opencc[key] = values.split()
        self.unihan = collections.defaultdict(dict)
        wanted = {"kMandarin", "kJapaneseOldVariant", "kZVariant", "kSemanticVariant", "kTraditionalVariant",
                  "kSpecializedSemanticVariant"}
        for name in ("Unihan_Readings.txt", "Unihan_Variants.txt"):
            with (cache / "unihan" / name).open(encoding="utf-8") as f:
                for line in f:
                    if line.startswith("#") or not line.strip():
                        continue
                    cp, field, value = line.rstrip("\n").split("\t", 2)
                    if field in wanted:
                        self.unihan[chr(int(cp[2:], 16))][field] = value
        self.resolve = functools.lru_cache(maxsize=None)(self.resolve)

    def field_chars(self, ch, field):
        out = []
        for token in self.unihan.get(ch, {}).get(field, "").split():
            m = re.match(r"U\+([0-9A-F]+)", token)
            if m:
                c = chr(int(m.group(1), 16))
                c = unicodedata.normalize("NFC", c) if 0xF900 <= ord(c) <= 0xFAFF else c
                if c != ch:
                    out.append(c)
        return out

    def unihan_mandarin(self, ch):
        value = self.unihan.get(ch, {}).get("kMandarin")
        return strip_tone(value.split()[0]) if value else None

    def old_forms(self, ch):
        forms = [t for t in self.opencc.get(ch, []) if t != ch]
        for c in self.field_chars(ch, "kJapaneseOldVariant"):
            if c not in forms:
                forms.append(c)
        return forms

    def project_char(self, form):
        """The project's reading of a Chinese character: mimo-sc first, then a clear mimo-tc winner."""
        hans = self.hant_to_hans.get(form, form)
        for cand in (hans, form):
            if cand in self.sc_char and self.sc_char[cand][1] > 1e-11:
                return self.sc_char[cand][0], "mimo-sc"
        cands = self.tc_char.get(form)
        if cands:
            top = max(cands.values())
            winners = [p for p, w in cands.items() if w == top]
            if len(winners) == 1 and top > 1:
                return winners[0], "mimo-tc"
        return None, None

    def resolve(self, ch):
        """(reading, route) of one kanji; (None, None) when no source has it."""
        for form in self.old_forms(ch):
            p, how = self.project_char(form)
            if p:
                return p, f"old-form+{how}"
        p, how = self.project_char(ch)
        if p:
            return p, how
        for field in ("kZVariant", "kSemanticVariant", "kTraditionalVariant", "kSpecializedSemanticVariant"):
            for var in self.field_chars(ch, field):
                p, how = self.project_char(var)
                if p:
                    return p, f"unihan-variant+{how}"
        for form in self.old_forms(ch):
            p = self.unihan_mandarin(form)
            if p:
                return p, "old-form+unihan"
        p = self.unihan_mandarin(ch)
        if p:
            return p, "unihan-kMandarin"
        hans = self.hant_to_hans.get(ch, ch)
        for cand in (hans, ch):
            if cand in self.sc_char:
                return self.sc_char[cand][0], "mimo-sc-rare"
        return None, None

    def char_forms(self, ch):
        forms = self.old_forms(ch) + [ch]
        for field in ("kZVariant", "kSemanticVariant", "kTraditionalVariant"):
            for var in self.field_chars(ch, field):
                if var not in forms and var in self.tc_char:
                    forms.append(var)
        return forms[:4]

    def word(self, word):
        """Reading of the Chinese entry with the same characters, or None."""
        options = [self.char_forms(c) for c in word]
        total = 1
        for o in options:
            total *= len(o)
        if total > 256:
            options = [o[:2] for o in options]
        for combo in itertools.product(*options):
            trad = "".join(combo)
            hans = "".join(self.hant_to_hans.get(c, c) for c in trad)
            for key, table in ((hans, self.sc_word), (trad, self.tc_word)):
                if key in table:
                    reading = max(table[key].items(), key=lambda kv: kv[1])[0].split()
                    if len(reading) == len(word):
                        return reading
        return None

    def chars(self, surface, star=True):
        """One syllable per kanji; 々 repeats the kanji before it. None marks a kanji without a reading."""
        out = []
        for ch in surface:
            if ch == REPEAT_MARK:
                out.append(out[-1] if out else None)
            elif KANJI.match(ch):
                p, how = self.resolve(ch)
                out.append(p + "*" if p and star and how in GUESS_ROUTES else p)
        return out

    def surface(self, surface):
        """(column 4, column 8, per-character reading when the word rule changed it)."""
        if not KANJI.search(surface):
            return "", "", None
        per_char = self.chars(surface)
        if None in per_char:
            raise SystemExit(f"no reading for a kanji of {surface}")
        if all(KANJI.match(c) for c in surface):
            reading = self.word(surface)
            if reading:
                plain = [p.rstrip("*") for p in per_char]
                return " ".join(reading), "word", (" ".join(per_char) if reading != plain else None)
        return " ".join(per_char), "char", None

    def polyphone_readings(self, ch):
        """mimo-tc single-character readings of the kanji after the old-form mapping, heaviest first."""
        for form in self.old_forms(ch) + [ch]:
            readings = self.tc_char.get(form)
            if readings:
                return form, sorted(readings.items(), key=lambda kv: (-kv[1], kv[0]))
        return ch, []


def reviewed_rows():
    """The 60 reviewed rows: Resources/Lexicon/japanese-reviewed.tsv, or the old snapshot before step C2."""
    for path in (ROOT / "Resources/Lexicon/japanese-reviewed.tsv", SNAPSHOT_DIR / "mimo-jp.tsv"):
        if path.is_file():
            rows = [line.split("|") for line in path.read_text(encoding="utf-8").splitlines()
                    if line and not line.startswith("#")]
            if rows and all(len(r) == 5 for r in rows):
                return path, rows
    return None, []


# --------------------------------------------------------------------------- build

def load_chinese():
    sc_path, tc_path = ROOT / "DataSources/mimo-sc/mimo-sc.json", ROOT / "DataSources/mimo-tc/mimo-tc.json"
    sc_entries = json.loads(sc_path.read_text(encoding="utf-8"))["entries"]
    tc_bytes = tc_path.read_bytes()
    entries_sha = sha256_bytes(json.dumps(sc_entries, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
    return sc_entries, json.loads(tc_bytes.decode("utf-8"))["entries"], entries_sha, sha256_bytes(tc_bytes)


def dump_json(value):
    return (json.dumps(value, ensure_ascii=False, indent=1) + "\n").encode("utf-8")


def source_record(cost_cut, entries_sha, tc_sha, files):
    def inputs(prefix, strip, names=None):
        return {strip + path[len(prefix):]: sha for path, sha, _, _ in INPUTS
                if path.startswith(prefix) and (names is None or path[len(prefix):] in names)}
    mozc_names = {f"dictionary{i:02d}.txt" for i in range(10)} | {
        "connection_single_column.txt", "id.def", "reading_correction.tsv"}
    return {
        "name": "mimo-jp",
        "version": VERSION,
        "license": "BSD-3-Clause AND NAIST-2003 AND Apache-2.0 AND Unicode-3.0 AND MIT",
        "generated_by": "scripts/mimo_jp_pipeline.py build",
        "parameters": {"cost_cut": cost_cut, "bc_gap": BC_GAP, "place_cut": PLACE_CUT, "stem_penalty": STEM_PENALTY, "class_offsets": CLASS_OFFSETS,
                       "weight": WEIGHT_TEXT},
        "upstreams": [
            {"name": "Mozc dictionary_oss", "repo": "https://github.com/google/mozc", "revision": MOZC_REV,
             "license": "BSD-3-Clause AND NAIST-2003", "inputs": inputs("mozc/", MOZC_DIR, mozc_names)},
            {"name": "UniDic unidic-cwj", "url": UNIDIC_URL, "version": "2025.12",
             "license": "BSD-3-Clause (chosen from GPL-2.0, LGPL-2.1, BSD-3-Clause)",
             "archive_sha256": UNIDIC_ARCHIVE_SHA256, "inputs": inputs("unidic-cwj-202512/", "", {"lex.csv"})},
            {"name": "OpenCC", "repo": "https://github.com/BYVoid/OpenCC", "revision": OPENCC_REV,
             "license": "Apache-2.0",
             "inputs": inputs("opencc/", "data/dictionary/", {"JPShinjitaiCharacters.txt"})},
            {"name": "Unihan", "url": UNIHAN_URL, "version": "18.0.0", "license": "Unicode-3.0",
             "archive_sha256": INPUT_SHA["unihan/Unihan.zip"],
             "inputs": inputs("unihan/", "", {"Unihan_Readings.txt", "Unihan_Variants.txt"})},
            {"name": "mimo-sc entries", "path": "DataSources/mimo-sc/mimo-sc.json", "license": "MIT",
             "notice": "DataSources/mimo-sc/LICENSE (pinyin-pro)", "entries_sha256": entries_sha,
             "note": "only `entries` is read and only `entries` is hashed: "
                     "sha256 of json.dumps(entries, ensure_ascii=False, separators=(',', ':'))"},
            {"name": "mimo-tc", "path": "DataSources/mimo-tc/mimo-tc.json", "license": "MIT AND Apache-2.0",
             "notice": "DataSources/mimo-tc/LICENSE.txt (McBopomofoData)", "sha256": tc_sha},
        ],
        "references": [
            {"name": "TUBELEX-JA", "repo": "https://github.com/naist-nlp/tubelex",
             "revision": "7cb5fb36add76b83a266d1967536e1a1d3faa513", "license": "BSD-3-Clause",
             "use": "five class offsets kept as constants; not read by the build"},
            {"name": "JMdict", "url": "http://ftp.edrdg.org/pub/Nihongo/JMdict_e.gz",
             "version": "file dated 2026-11-10",
             "sha256": "8e511b59c1efb57d7dc90f58f386058035ca662cb4a23d5693b5e185190c5ede",
             "license": "CC-BY-SA-4.0",
             "use": "chose which rows a reviewer read first for Resources/Lexicon/japanese-deny.tsv; "
                    "not read by the build, nothing copied"},
            {"name": "SudachiDict",
             "url": "https://sudachi.s3.ap-northeast-1.amazonaws.com/sudachidict-raw/v1/20261015/",
             "version": "20261015",
             "sha256": {"small_lex.zip": "981b78cdc532ce058b34c76cd9acd3c9a6c771cc0ce528d7de92424b21172125",
                        "core_lex.zip": "e9ac06222f0ce401a0fef9b129e61e93d59e3ff395a1b037f498962ee5b8ad8b",
                        "notcore_lex.zip": "16072d75abe6a048947aacbcecfe1ab65a200965a9f1ddb7ceab366348fa21f2"},
             "license": "Apache-2.0", "use": "same as JMdict"},
        ],
        "history": "Versions 1.0-1.1 held 60 hand-picked rows, now in Resources/Lexicon/japanese-reviewed.tsv. "
                   "Their record named two checksums that no script verified and that match no 2025.12 file. "
                   "Old sources: https://clrd.ninjal.ac.jp/unidic/ and "
                   "https://github.com/google/mozc (UniDic-to-Mozc table v1.1.0), see commit d4b6136.",
        "files": files,
    }


def build(cache, out, cost_cut, dump_r6=None):
    """Write the snapshot into `out`. Returns the lines printed for the step notes."""
    started = time.time()
    require_inputs(cache)
    stats = collections.Counter()
    unidic = UniDic(cache)
    say(f"UniDic read ({time.time() - started:.0f} s)")
    wide = select_wide(cache, unidic, stats, cost_cut)
    rows = cut_rows(wide, cost_cut)
    say(f"selection done ({time.time() - started:.0f} s)")
    if dump_r6:
        with open(dump_r6, "w", encoding="utf-8") as f:
            for adjusted, key, value, cost, tier, cls, note in rows:
                f.write("\t".join([key, value, str(adjusted), str(cost), tier, cls, note,
                                   str(prototype_weight(adjusted))]) + "\n")

    counts = collections.OrderedDict()
    counts["mozc_rows"] = stats["mozc rows"]
    counts["r0_dropped"] = stats["R0 dropped"]
    counts["r1_r2_dropped"] = stats["R1/R2 dropped"]
    counts["pairs_before_cut"] = len(wide)
    counts["after_r6_with_u3006"] = len(rows)
    rows = [r for r in rows if CLOSING_MARK not in r[2]]
    counts["r0_u3006_dropped"] = counts["after_r6_with_u3006"] - len(rows)
    counts["after_r6"] = len(rows)

    r7 = voiced_duplicates(rows)
    rows = [r for r in rows if (r[1], r[2]) not in r7]
    counts["r7_dropped"], counts["after_r7"] = len(r7), len(rows)
    r8_hira, r8_kata, r8_kept = kana_duplicates(rows, unidic.loanwords)
    counts["r8_hiragana_dropped"] = len(r8_hira)
    counts["r8_katakana_dropped"] = len(r8_kata)
    counts["r8_katakana_kept_as_loanword"] = len(r8_kept)
    best_of = {}
    for r in rows:
        best_of.setdefault(r[1], r[2])
    rows = [r for r in rows if (r[1], r[2]) not in r8_hira and (r[1], r[2]) not in r8_kata]
    counts["rows"] = len(rows)
    counts["tiers"] = {t: sum(1 for r in rows if r[4] == t) for t in "ABC"}

    sc_entries, tc_entries, entries_sha, tc_sha = load_chinese()
    pinyin = Pinyin(cache, sc_entries, tc_entries)
    weights = {cost: weight_of(cost) for cost in sorted({r[0] for r in rows})}

    lines, word_diffs, unknown, no_romaji = [], [], [], []
    kanji_words, kanji_examples = collections.Counter(), collections.defaultdict(list)
    class_counts, rule_counts = collections.Counter(), collections.Counter()
    for adjusted, key, value, cost, tier, cls, note in rows:   # by weight, so examples are common words
        if cls == "verb":
            ctypes = unidic.ctypes.get((key, value), ()) if tier == "A" else ()
            name = conjugation(ctypes)
            if name == "v?" and tier == "A":
                unknown.append([key, value, sorted(ctypes)])
        elif cls == "adj":
            # An i-adjective only when UniDic says so (tier A) and the word ends in い; classical
            # forms, spoken forms (すげえ) and words outside UniDic get adj? and no generated forms.
            ctypes = unidic.ctypes.get((key, value), ()) if tier == "A" else ()
            name = "adj-i" if "形容詞" in ctypes and value.endswith("い") else "adj?"
            if name == "adj?" and tier == "A":
                unknown.append([key, value, sorted(ctypes)])
        else:
            name = CLASS_NAMES[cls]
        class_counts[name] += 1
        romaji = kana_to_hepburn(key)
        if not romaji:
            no_romaji.append([key, value])
        column4, rule, per_char = pinyin.surface(value)
        rule_counts[rule or "none"] += 1
        if per_char is not None:
            word_diffs.append([key, value, column4, per_char])
        for ch in sorted(set(KANJI.findall(value))):
            kanji_words[ch] += 1
            if len(kanji_examples[ch]) < 3:
                kanji_examples[ch].append(value)
        lines.append((key, value, f"{key}|{value}|{romaji}|{column4}|{weights[adjusted]}|{name}|{tier}|{rule}"))
    lines.sort(key=lambda r: (r[0], r[1]))
    snapshot = ("# mimo-jp " + VERSION + ", generated by scripts/mimo_jp_pipeline.py build; do not edit. "
                "kana|surface|romaji|pinyin|weight|class|tier|pinyin-rule\n"
                + "".join(line + "\n" for _, _, line in lines)).encode("utf-8")

    table_lines, routes, guesses, polyphones = [], collections.Counter(), [], []
    for ch in sorted(kanji_words):
        reading, route = pinyin.resolve(ch)
        if not reading:
            raise SystemExit(f"no reading for {ch}")
        routes[route] += 1
        if route in GUESS_ROUTES:
            reading += "*"
            guesses.append([ch, reading, kanji_words[ch]])
        table_lines.append(f"{ch}\t{reading}\t{route}\t{''.join(pinyin.old_forms(ch))}\n")
        form, readings = pinyin.polyphone_readings(ch)
        if len(readings) >= 2:
            polyphones.append({"kanji": ch, "chinese_form": form, "default": reading,
                               "readings": [[p, w] for p, w in readings], "words": kanji_words[ch],
                               "examples": kanji_examples[ch]})
    polyphones.sort(key=lambda p: (-p["words"], p["kanji"]))
    guesses.sort(key=lambda g: (-g[2], g[0]))
    kanji_table = ("# mimo-jp " + VERSION + " kanji readings, generated by scripts/mimo_jp_pipeline.py build; "
                   "do not edit. kanji<TAB>pinyin<TAB>route<TAB>old form\n" + "".join(table_lines)).encode("utf-8")

    all_weights = [weights[r[0]] for r in rows]
    report = collections.OrderedDict()
    report["name"], report["version"] = "mimo-jp", VERSION
    report["parameters"] = {"cost_cut": cost_cut, "bc_gap": BC_GAP, "place_cut": PLACE_CUT,
                            "stem_penalty": STEM_PENALTY, "class_offsets": CLASS_OFFSETS, "weight": WEIGHT_TEXT}
    report["counts"] = counts
    report["selection_stats"] = {name: stats[name] for name in sorted(stats)}
    report["classes"] = {name: class_counts[name] for name in sorted(class_counts)}
    report["weights"] = {"max": max(all_weights), "min": min(all_weights), "distinct_costs": len(weights),
                         "rows_at_least": {str(t): sum(1 for w in all_weights if w >= t)
                                           for t in (6000, 5000, 4500, 4000, 3200, 2500)}}
    report["romaji"] = {"rows_without": len(no_romaji), "rows": sorted(no_romaji)}
    report["pinyin"] = {
        "kanji": len(table_lines), "routes": {name: routes[name] for name in sorted(routes)},
        "rows_by_rule": {name: rule_counts[name] for name in sorted(rule_counts)},
        "unihan_guesses": guesses,
        "polyphones_listed": len(polyphones),
        "polyphones_in_20_words_or_more": sum(1 for p in polyphones if p["words"] >= 20),
        "polyphones_in_50_words_or_more": sum(1 for p in polyphones if p["words"] >= 50),
        "word_level_differences": len(word_diffs)}
    report["review"] = collections.OrderedDict([
        ("note", "polyphones: kanji with two or more single-character readings in mimo-tc after the old-form "
                 "mapping; `default` is what column 4 uses. word_level_differences: kana, surface, reading of "
                 "the Chinese entry, per-character reading."),
        ("polyphones", polyphones),
        ("word_level_differences", sorted(word_diffs)),
        ("conjugation_unknown_tier_a", sorted(unknown)),
        ("r7_voiced_duplicates", sorted(map(list, r7))),
        ("r8_katakana_dropped", sorted(map(list, r8_kata))),
        ("r8_katakana_kept_as_loanword", sorted(map(list, r8_kept))),
    ])

    licence_sources = {"LICENSE.txt": SNAPSHOT_DIR / "LICENSE.txt",
                       "BSD.txt": cache / "unidic-cwj-202512/license/BSD",
                       "COPYING.txt": cache / "unidic-cwj-202512/license/COPYING",
                       "MOZC-LICENSE.txt": cache / "mozc/LICENSE",
                       "UNICODE-LICENSE.txt": cache / "unihan/license.txt"}
    files = collections.OrderedDict([("mimo-jp.tsv", sha256_bytes(snapshot)),
                                     ("kanji-pinyin.tsv", sha256_bytes(kanji_table))])
    for name, path in licence_sources.items():
        files[name] = sha256_file(path)
    if files["LICENSE.txt"] != EXISTING_LICENSE_SHA256:
        raise SystemExit("DataSources/mimo-jp/LICENSE.txt is not the pinned UniDic notice")
    source = source_record(cost_cut, entries_sha, tc_sha, files)

    out.mkdir(parents=True, exist_ok=True)
    (out / "mimo-jp.tsv").write_bytes(snapshot)
    (out / "kanji-pinyin.tsv").write_bytes(kanji_table)
    (out / "build-report.json").write_bytes(dump_json(report))
    (out / "SOURCE.json").write_bytes(dump_json(source))
    for name in ("MOZC-LICENSE.txt", "UNICODE-LICENSE.txt"):
        shutil.copyfile(licence_sources[name], out / name)

    # Console only: these depend on project files or on the machine, so they stay out of the report.
    path, reviewed = reviewed_rows()
    pairs = {(key, value) for key, value, _ in lines}
    same, differ = 0, []
    for kana, word, _, chinese, _ in reviewed:
        auto = pinyin.word(word) if len(word) >= 2 and all(KANJI.match(c) for c in word) else None
        auto = auto or pinyin.chars(word, star=False)
        if auto == chinese.split():
            same += 1
        else:
            differ.append(f"{word}: reviewed {chinese} / chain {' '.join(a or '?' for a in auto)}")
    say(f"rows {counts['rows']} (A {counts['tiers']['A']}, B {counts['tiers']['B']}, C {counts['tiers']['C']}); "
        f"after R6 {counts['after_r6_with_u3006']} with 〆, {counts['after_r6']} without; R7 -{len(r7)}; "
        f"R8 hiragana -{len(r8_hira)}, katakana -{len(r8_kata)} (kept {len(r8_kept)})")
    say(f"weights {min(all_weights)}..{max(all_weights)}; rows without romaji {len(no_romaji)}; "
        f"kanji {len(table_lines)}, Unihan guesses {len(guesses)}; polyphones {len(polyphones)} "
        f"({report['pinyin']['polyphones_in_20_words_or_more']} in 20 words or more); "
        f"word-level differences {len(word_diffs)}")
    if reviewed:
        say(f"reviewed rows ({path.relative_to(ROOT)}): {len(reviewed)}; column 4 reproduced for {same}; "
            f"pair also in the snapshot for {sum(1 for r in reviewed if (r[0], r[1]) in pairs)}")
        for line in differ:
            say("  " + line)
    say(f"wrote {out} in {time.time() - started:.0f} s")
    return len(differ)


def cmd_build(args):
    differ = build(pathlib.Path(args.cache), pathlib.Path(args.out), args.cost_cut, args.dump_r6)
    return 1 if differ and args.require_reviewed else 0


def cmd_verify(args):
    with tempfile.TemporaryDirectory(prefix="mimo-jp-verify-") as temp:
        build(pathlib.Path(args.cache), pathlib.Path(temp), COST_CUT)
        bad = []
        for name in GENERATED:
            committed = SNAPSHOT_DIR / name
            if not committed.is_file():
                bad.append(f"{name}: missing in DataSources/mimo-jp")
            elif committed.read_bytes() != (pathlib.Path(temp) / name).read_bytes():
                bad.append(f"{name}: differs from the rebuilt file")
    for line in bad:
        say("FAIL " + line)
    if not bad:
        say("DataSources/mimo-jp matches a fresh build")
    return 1 if bad else 0


def main():
    parser = argparse.ArgumentParser(description="mimo-jp data pipeline")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("fetch", help="download missing inputs and verify every sha256")
    p.add_argument("--cache", default=str(DEFAULT_CACHE))
    p.add_argument("--seed", help="directory with already downloaded inputs in the cache layout")
    p.add_argument("--jobs", type=int, default=8, help="parallel range requests")
    p.set_defaults(run=cmd_fetch)
    p = sub.add_parser("build", help="write the snapshot files into --out")
    p.add_argument("--cache", default=str(DEFAULT_CACHE))
    p.add_argument("--out", default=str(DEFAULT_OUT))
    p.add_argument("--cost-cut", type=int, default=COST_CUT, help="R6 cut on the adjusted cost of tier A")
    p.add_argument("--dump-r6", metavar="FILE",
                   help="also write the rows after R6 in the format of the research prototype")
    p.add_argument("--require-reviewed", action="store_true",
                   help="exit 1 when a reviewed row does not reproduce its pinyin column")
    p.set_defaults(run=cmd_build)
    p = sub.add_parser("verify", help="rebuild and compare with DataSources/mimo-jp")
    p.add_argument("--cache", default=str(DEFAULT_CACHE))
    p.set_defaults(run=cmd_verify)
    args = parser.parse_args()
    if args.command == "build" and pathlib.Path(args.out).resolve() == SNAPSHOT_DIR.resolve():
        parser.error("build does not write into DataSources/mimo-jp; copy the files in step C2")
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
