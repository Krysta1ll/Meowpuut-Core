#!/usr/bin/env python3
"""Packs DataSources/mimo-english into the English lexicon image mimo_en.bin (MIMO-EN-1).

  python3 -I scripts/mimo_en_binary.py [OUT] [--english FILE] [--pairs FILE] [--limit N] [--dump FILE]

OUT defaults to .build/lexicon/mimo_en.bin. FILE may be .xz or plain TSV (the format of
scripts/mimo_english.py). The output depends only on the two files and the limit, and
crates/meowpuut-core/src/english.rs (`pack`) writes the same bytes from the same input
(tests/english.rs compares them). --dump writes lookups of the packed file for that test.

The never-volunteer list. A word of the [en] section of Resources/Lexicon/blocked-terms.txt
(scripts/blocked_terms.py) gets the flag nosuggest here as well, and every word pair with such a
word on either side is left out, as is a listed word pair (left_right in [en]: Kim -> Jong), so a
word added to the list is never suggested without counting the text again (main(); `pack` takes
the rows as they are).

Rows. Every row of the word list that is in Chinese text (in-chinese) or never suggested
(nosuggest) is kept, then the most frequent others until there are --limit rows (default
50,000, the iOS budget of the spec; the list has about 58,000). Rows are sorted by key
(bytes), heaviest first within a key, then by spelling.

Weights. On the lexicon's scale: weight = round(700 (ln(count / total) + 19)), total being
the sum of the counts of the whole list, clamped to 0..65535; score = weight / 700 - 19.

Word pairs. Ids: 0 the sentence start, 1 the end, row + 2 a word. The score of b after a
is 0.8 ln(P(b|a) / P(b)) in 1/64 nat, P(b|a) = count(a b) / count(a), clamped to 12 bits
signed, as in MIMO-PAIR-1. A pair that is not stored scores the backoff of a:
0.8 ln((1 - sum of stored P(b|a)) / (1 - sum of their P(b))), never above 0. The next
words of a are its eight most frequent followers that may be suggested.

Layout (little-endian; sections start 8-byte aligned, zero padding):

  0    magic "MIMO-EN-1\\0\\0\\0"
  12   version u32 (1), word count n u32, pair count p u32, next-word count q u32
  28   fingerprint u64: FNV-1a 64 of the keys, the spellings and the weights
  36   common weight u32: weight of the 30,000th heaviest row
  40   section count u32 (13)
  44   13 x (offset u32, length u32)
  sections:
   0 key offsets (n+1) u32     1 keys (lower-case ASCII letters, sorted)
   2 spelling offsets (n+1) u32 (an empty spelling is the key)
   3 spellings (UTF-8)          4 weights n u16
   5 flags n u8: 1 NO_SUGGEST, 2 SOFT, 4 IN_CHINESE, 32 PROPER, 64 ACRONYM, 128 MIXED
   6 nine-key order n u32: rows sorted by the keypad digits of their key, heaviest first
   7 first-letter top 26 x 16 u32: heaviest suggestible rows of each first letter (0xFFFFFFFF: none)
   8 pair offsets (n+3) u32     9 backoff (n+2) i16
  10 pairs p u32: follower id << 12 | 12-bit score, sorted by follower id
  11 next-word offsets (n+3) u32
  12 next words q u32 (rows)
"""
import argparse
import lzma
import math
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-english"
MAGIC = b"MIMO-EN-1\0\0\0"
VERSION = 1
LIMIT = 50000
COMMON = 30000
PREDICT = 8
FIRST_TOP = 16
SECTIONS = 13
HEADER = 160
NONE = 0xFFFFFFFF
FLAG_BITS = {"nosuggest": 1, "soft": 2, "in-chinese": 4, "proper": 32, "acronym": 64, "mixed": 128}
NO_OFFER = 1 | 2
DIGITS = b"22233344455566677778889999"


def read_tsv(path):
    path = Path(path)
    raw = lzma.open(path, "rb").read() if path.suffix == ".xz" else path.read_bytes()
    return [line.split("\t") for line in raw.decode("utf-8").split("\n") if line and not line.startswith("#")]


def weight_of(count, total):
    return max(0, min(65535, math.floor(700.0 * (math.log(count / total) + 19.0) + 0.5)))


def fnv(data, h=0xCBF29CE484222325):
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def blocked_list():
    """scripts/blocked_terms.py, loaded by path (this script runs with python3 -I)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("blocked_terms", ROOT / "scripts/blocked_terms.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def never_volunteer(english, pairs):
    """The rows with nosuggest added to the words of the [en] list, and the pairs without them."""
    listed = blocked_list()
    marked, rows = set(), []
    for fields in english:
        fields = list(fields)
        if listed.en_hit(fields[1]) or listed.en_hit(fields[0]):
            flags = [f for f in fields[3].split(",") if f and f != "-"]
            if "nosuggest" not in flags:
                fields[3] = ",".join(sorted(flags + ["nosuggest"]))
            marked.add(fields[1])
        rows.append(fields)
    kept = [p for p in pairs if p[0] not in marked and p[1] not in marked and not listed.en_pair_hit(p[0], p[1])]
    return rows, kept, len(marked), len(pairs) - len(kept)


def pack(english, pairs, limit=LIMIT):
    """Bytes of mimo_en.bin for the rows of english.tsv and pairs.tsv (lists of fields)."""
    words = []
    for fields in english:
        key, spelling, count, flags = fields[0], fields[1], int(fields[2]), fields[3]
        bits = 0
        for flag in flags.split(","):
            bits |= FLAG_BITS.get(flag, 0)
        if not key.isascii() or not key.isalpha() or not key.islower() or count <= 0:
            raise SystemExit(f"mimo_en_binary: bad row {fields}")
        words.append((key, spelling, count, bits))
    total = sum(w[2] for w in words)
    kept = [w for w in words if w[3] & (1 | 4)]
    others = sorted((w for w in words if not w[3] & (1 | 4)), key=lambda w: (-w[2], w[0].encode(), w[1].encode()))
    kept += others[:max(0, limit - len(kept))]
    rows = sorted(((w[0], w[1], w[2], w[3], weight_of(w[2], total)) for w in kept),
                  key=lambda r: (r[0].encode(), -r[4], r[1].encode()))
    n = len(rows)
    row_of = {r[1]: i for i, r in enumerate(rows)}

    keys, key_offsets = bytearray(), [0]
    spellings, spelling_offsets = bytearray(), [0]
    for key, spelling, _, _, _ in rows:
        keys += key.encode()
        key_offsets.append(len(keys))
        if spelling != key:
            spellings += spelling.encode()
        spelling_offsets.append(len(spellings))
    weights = struct.pack(f"<{n}H", *(r[4] for r in rows))
    flags = bytes(r[3] for r in rows)

    t9 = sorted(range(n), key=lambda i: (bytes(DIGITS[b - 97] for b in rows[i][0].encode()), -rows[i][4], i))
    first = []
    for letter in range(26):
        best = sorted((i for i in range(n) if rows[i][0].encode()[0] == 97 + letter and not rows[i][3] & NO_OFFER),
                      key=lambda i: (-rows[i][4], i))[:FIRST_TOP]
        first += best + [NONE] * (FIRST_TOP - len(best))
    common = sorted((r[4] for r in rows), reverse=True)[min(COMMON, n) - 1] if n else 0

    # Word pairs, by id: 0 start, 1 end, row + 2.
    start_total = sum(int(p[2]) for p in pairs if p[0] == "<s>")
    end_total = sum(int(p[2]) for p in pairs if p[1] == "</s>")
    followers = [[] for _ in range(n + 2)]
    for left, right, count in pairs:
        a = 0 if left == "<s>" else (row_of[left] + 2 if left in row_of else None)
        b = 1 if right == "</s>" else (row_of[right] + 2 if right in row_of else None)
        if a is None or b is None or a == 1:
            continue
        followers[a].append((b, int(count)))
    left_count = [start_total, 0] + [r[2] for r in rows]
    right_count = [0, end_total] + [r[2] for r in rows]
    pair_offsets, backoff, packed = [0], [], []
    predict_offsets, predict = [0], []
    for a in range(n + 2):
        items = sorted(followers[a])
        sum_p, sum_q = 0.0, 0.0
        for b, count in items:
            pba = min(1.0, count / left_count[a]) if left_count[a] else 1.0
            pb = right_count[b] / total
            score = max(-2048, min(2047, math.floor(0.8 * math.log(pba / pb) * 64.0 + 0.5)))
            packed.append((b << 12) | (score & 0xFFF))
            sum_p += pba
            sum_q += pb
        if items:
            x = 0.8 * math.log(max(1e-3, 1.0 - sum_p) / max(1e-3, 1.0 - sum_q)) * 64.0
            backoff.append(max(-2048, min(0, math.floor(x + 0.5))))
        else:
            backoff.append(0)
        pair_offsets.append(len(packed))
        best = sorted(((b, count) for b, count in items if b >= 2 and not rows[b - 2][3] & NO_OFFER),
                      key=lambda bc: (-bc[1], bc[0]))[:PREDICT]
        predict += [b - 2 for b, _ in best]
        predict_offsets.append(len(predict))

    sections = [
        struct.pack(f"<{n + 1}I", *key_offsets), bytes(keys),
        struct.pack(f"<{n + 1}I", *spelling_offsets), bytes(spellings),
        weights, flags,
        struct.pack(f"<{n}I", *t9), struct.pack(f"<{26 * FIRST_TOP}I", *first),
        struct.pack(f"<{n + 3}I", *pair_offsets), struct.pack(f"<{n + 2}h", *backoff),
        struct.pack(f"<{len(packed)}I", *packed),
        struct.pack(f"<{n + 3}I", *predict_offsets), struct.pack(f"<{len(predict)}I", *predict),
    ]
    assert len(sections) == SECTIONS
    fingerprint = fnv(weights, fnv(spellings, fnv(keys)))
    body, table = bytearray(), []
    for section in sections:
        offset = HEADER + len(body)
        table.append((offset, len(section)))
        body += section
        body += b"\0" * (-len(body) % 8)
    header = bytearray(MAGIC)
    header += struct.pack("<IIIIQII", VERSION, n, len(packed), len(predict), fingerprint, common, SECTIONS)
    for offset, length in table:
        header += struct.pack("<II", offset, length)
    header += b"\0" * (HEADER - len(header))
    return bytes(header + body)


class Reader:
    """Lookups of a packed image, for --dump (the same questions tests/english.rs asks the core)."""

    def __init__(self, data):
        self.data = data
        self.n, self.p, self.q = struct.unpack_from("<III", data, 16)
        self.common = struct.unpack_from("<I", data, 36)[0]
        self.sec = [struct.unpack_from("<II", data, 44 + 8 * i) for i in range(SECTIONS)]

    def u32(self, s, i):
        return struct.unpack_from("<I", self.data, self.sec[s][0] + 4 * i)[0]

    def key(self, row):
        a, b = self.u32(0, row), self.u32(0, row + 1)
        return self.data[self.sec[1][0] + a:self.sec[1][0] + b].decode()

    def spelling(self, row):
        a, b = self.u32(2, row), self.u32(2, row + 1)
        return self.data[self.sec[3][0] + a:self.sec[3][0] + b].decode() if b > a else self.key(row)

    def weight(self, row):
        return struct.unpack_from("<H", self.data, self.sec[4][0] + 2 * row)[0]

    def score(self, a, b):
        lo, hi = self.u32(8, a), self.u32(8, a + 1)
        for i in range(lo, hi):
            pair = self.u32(10, i)
            if pair >> 12 == b:
                s = pair & 0xFFF
                return s - 4096 if s >= 2048 else s
        return struct.unpack_from("<h", self.data, self.sec[9][0] + 2 * a)[0]

    def predict(self, a):
        return [self.u32(12, i) for i in range(self.u32(11, a), self.u32(11, a + 1))]

    def t9(self, digits):
        rows = [self.u32(6, i) for i in range(self.n)]
        return [r for r in rows if "".join(chr(DIGITS[ord(c) - 97]) for c in self.key(r)).startswith(digits)]


def dump(data, path):
    """Lines the Rust test checks: row spellings and weights, pair scores, next words, nine-key rows."""
    r = Reader(data)
    lines = [f"n\t{r.n}\t{r.p}\t{r.q}\t{r.common}"]
    step = max(1, r.n // 400)
    for row in range(0, r.n, step):
        lines.append(f"row\t{row}\t{r.key(row)}\t{r.spelling(row)}\t{r.weight(row)}")
        lines.append("next\t%d\t%s" % (row + 2, ",".join(map(str, r.predict(row + 2)))))
        for other in (0, 1, (row * 7919) % r.n):
            lines.append(f"pair\t{row + 2}\t{other + 2 if other > 1 else other}\t{r.score(row + 2, other + 2 if other > 1 else other)}")
    lines.append("next\t0\t%s" % ",".join(map(str, r.predict(0))))
    for digits in ("65", "474663", "4663", "843", "3668", "2"):
        lines.append("t9\t%s\t%s" % (digits, ",".join(map(str, r.t9(digits)[:20]))))
    Path(path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("out", nargs="?", default=str(ROOT / ".build/lexicon/mimo_en.bin"))
    parser.add_argument("--english", default=str(DATA / "english.tsv.xz"))
    parser.add_argument("--pairs", default=str(DATA / "pairs.tsv.xz"))
    parser.add_argument("--limit", type=int, default=LIMIT)
    parser.add_argument("--dump")
    args = parser.parse_args()
    english, pairs, marked, dropped = never_volunteer(read_tsv(args.english), read_tsv(args.pairs))
    print(f"mimo_en_binary: never-volunteer list: {marked} words marked nosuggest, {dropped} pairs left out")
    data = pack(english, pairs, args.limit)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(data)
    r = Reader(data)
    print(f"mimo_en_binary: {out.name}: {len(data):,} bytes; {r.n:,} words, {r.p:,} pairs, {r.q:,} next words")
    if args.dump:
        dump(data, args.dump)
    return 0


if __name__ == "__main__":
    sys.exit(main())
