#!/usr/bin/env python3
"""Japanese next-word table: which typed unit follows which, counted in openly licensed text.

After a Japanese word is committed the keyboard offers what may follow it (ありがとう → ございます,
今日 → は). The engine reads those from mimo_jp_next.bin (crates/meowpuut-core/src/japanese_next.rs).
This script makes that table.

A build only packs: scripts/build-lexicon.sh calls `pack`, which reads the committed counts
(DataSources/mimo-jp-next/next.tsv.xz) and writes mimo_jp_next.bin. The counts are redone by hand
when the sources or the splitting change:

  python3 -I scripts/mimo_jp_next.py fetch
      Downloads the sentence files into .build/jp-next/dl and checks SHA-256 against
      DataSources/mimo-jp-next/SOURCE.json. The Mozc dictionary files come from the cache of
      scripts/mimo_jp_pipeline.py (.build/mimo-jp-cache/mozc, same pinned revision and checksums).
  python3 -I scripts/mimo_jp_next.py count [--jobs 8]
      Splits every clause into words with the Mozc dictionary and its connection costs (the most
      likely path, as MeCab does), joins the words into the units a person commits on a keyboard,
      and counts which unit follows which, per source (.build/jp-next/counts-<source>.tsv).
  python3 -I scripts/mimo_jp_next.py snapshot
      Merges the counts with the weight of each source, keeps the pairs worth offering and writes
      DataSources/mimo-jp-next/next.tsv.xz and counts.json.
  python3 -I scripts/mimo_jp_next.py pack OUT
      Writes OUT (mimo_jp_next.bin, format MIMO-JN-1) from the committed snapshot.

Units. A unit is what one conversion commits: a content word with what inflects it
(食べ+まし+た is 食べました, 天気+です is 天気です, お+願い+し+ます is お願いします), or one particle
(は, を, から). For every unit the table also learns from its inflecting tail alone (ました, です),
so a word the corpus never saw still gets the continuations of its ending.

Keys. A key is the text of one unit, of a tail, or of two units in a row (ご飯+を). The engine
looks up the last two committed pieces together, then the last one, then the longest tail the
last one ends with (only keys flagged TAIL may match as an ending).

A clause with a term of Resources/Lexicon/blocked-terms.txt [ja] is not counted, and no such
term is packed.

Python standard library only.
"""
import argparse
import array
import bz2
import collections
import hashlib
import json
import lzma
import multiprocessing
import re
import struct
import subprocess
import sys
import unicodedata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # python3 -I leaves the script directory out
import blocked_terms  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-jp-next"
SNAPSHOT = DATA / "next.tsv.xz"
CACHE = ROOT / ".build/jp-next"
MOZC = ROOT / ".build/mimo-jp-cache/mozc"

MAGIC = b"MIMO-JN-1\0\0\0"
VERSION = 1
HEADER_LEN = 48
KEY_RECORD_LEN = 12
NEXT_RECORD_LEN = 8
FLAG_TAIL = 1
MAX_KEY_BYTES = 48
MAX_TEXT_BYTES = 48
MAX_NEXT_PER_KEY = 12

# Snapshot rules.
MIN_KEY_COUNT = 6       # weighted occurrences of a key
MIN_PAIR_COUNT = 3      # weighted occurrences of key -> next
MIN_SHARE = 0.004       # of the key's occurrences
MAX_UNIT_CHARS = 12
MAX_PAIR_KEY_CHARS = 8  # two units in a row are a key up to this length

# The example sentences of Tatoeba are about the same few people and their town; none of them
# is offered or learned from (a unit that starts with one of these names is left out).
CORPUS_NAMES = ("トム", "メアリー", "メアリ", "ジョン", "ジェーン", "ボブ", "ボストン", "ジム", "ビル", "ケン",
                "マイク", "ナンシー", "ケイト", "ベティ", "ジャック", "ジェーン", "メグ", "ジョージ", "スミス",
                "ブラウン", "ジャクソン", "田中", "山田", "鈴木", "佐藤", "健", "太郎", "花子")

JAPANESE_RUN = re.compile(r"[ぁ-ゖァ-ヺー一-鿿㐀-䶿々〆ヶ]+")
KATAKANA_RUN = re.compile(r"[ァ-ヺー]+")
MAX_WORD_CHARS = 12
UNKNOWN_COST = 11000
UNKNOWN_KATAKANA_COST = 8500
NOUN_ID = 1851          # 名詞,一般
RESPLIT_MARGIN = 2500   # a phrase entry is taken apart when its parts cost at most this much more

# 接続助詞 that belong to the word before them (食べて, 読んで, 行けば).
JOINING_PARTICLES = {"て", "で", "ば", "たり", "だり", "ながら", "つつ", "ちゃ", "じゃ"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def sources() -> dict:
    return json.loads((DATA / "SOURCE.json").read_text(encoding="utf-8"))["sources"]


def fetch():
    (CACHE / "dl").mkdir(parents=True, exist_ok=True)
    for name, src in sources().items():
        if "path" in src:
            continue
        path = CACHE / "dl" / src["cache"]
        if path.exists() and sha256(path) == src["sha256"]:
            print(f"{name}: present")
            continue
        tmp = path.with_suffix(path.suffix + ".part")
        subprocess.run(["curl", "-fL", "--retry", "3", "-o", str(tmp), src["url"]], check=True)
        if sha256(tmp) != src["sha256"]:
            raise SystemExit(f"{name}: SHA-256 differs from SOURCE.json. The file at {src['url']} changed; "
                             "check the new file and its terms, then update SOURCE.json.")
        tmp.replace(path)
        print(f"{name}: {path}")


# --------------------------------------------------------------------------- word splitting

class Splitter:
    """Most likely path through the Mozc dictionary words of a run of Japanese characters."""

    def __init__(self, mozc: Path = MOZC):
        self.ids = {}
        for line in (mozc / "id.def").read_text(encoding="utf-8").splitlines():
            number, name = line.split(" ", 1)
            self.ids[int(number)] = name.split(",")
        with (mozc / "connection_single_column.txt").open(encoding="ascii") as f:
            self.size = int(f.readline())
            # Line rid * size + lid after the size line.
            self.connection = array.array("h", (int(line) for line in f))
        if len(self.connection) != self.size * self.size:
            raise SystemExit("connection_single_column.txt is not a square matrix")
        best = {}
        for i in range(10):
            with (mozc / f"dictionary{i:02d}.txt").open(encoding="utf-8") as f:
                for line in f:
                    parts = line.rstrip("\n").split("\t")
                    surface = parts[4]
                    if len(surface) > MAX_WORD_CHARS or not JAPANESE_RUN.fullmatch(surface):
                        continue
                    key = (surface, int(parts[1]), int(parts[2]))
                    cost = int(parts[3])
                    if cost < best.get(key, 1 << 30):
                        best[key] = cost
        words = collections.defaultdict(list)
        for (surface, lid, rid), cost in best.items():
            words[surface].append((lid, rid, cost))
        self.words = {surface: tuple(rows) for surface, rows in words.items()}

    def split(self, run: str, whole: bool = True):
        """([(surface, left id)], cost) of the cheapest path; an unknown stretch is a noun.
        With `whole` false, no single word may cover the run."""
        n, size, connection, words = len(run), self.size, self.connection, self.words
        # states[i]: right id -> (cost, start, right id before, left id)
        states = [None] * (n + 1)
        states[0] = {0: (0, -1, 0, 0)}
        for i in range(n):
            here = states[i]
            if not here:
                continue
            arriving = {}

            def reach(lid, here=here, arriving=arriving, row_of=connection, size=size):
                found = arriving.get(lid)
                if found is None:
                    cost, before = 1 << 30, 0
                    for rid, state in here.items():
                        c = state[0] + row_of[rid * size + lid]
                        if c < cost:
                            cost, before = c, rid
                    found = arriving[lid] = (cost, before)
                return found

            known = False
            for length in range(1, min(MAX_WORD_CHARS, n - i) + 1):
                rows = words.get(run[i:i + length])
                if not rows:
                    continue
                known = True
                if length == n and not whole:
                    continue
                target = states[i + length]
                if target is None:
                    target = states[i + length] = {}
                for lid, rid, word_cost in rows:
                    cost, before = reach(lid)
                    cost += word_cost
                    old = target.get(rid)
                    if old is None or cost < old[0]:
                        target[rid] = (cost, i, before, lid)
            katakana = KATAKANA_RUN.match(run, i)
            if katakana and katakana.end() - i > 1 and (whole or katakana.end() - i < n):
                length, word_cost = katakana.end() - i, UNKNOWN_KATAKANA_COST
            elif not known:
                length, word_cost = 1, UNKNOWN_COST
            else:
                continue
            target = states[i + length]
            if target is None:
                target = states[i + length] = {}
            cost, before = reach(NOUN_ID)
            cost += word_cost
            old = target.get(NOUN_ID)
            if old is None or cost < old[0]:
                target[NOUN_ID] = (cost, i, before, NOUN_ID)
        last = states[n]
        if not last:
            return [], 1 << 30
        rid = min(last, key=lambda r: last[r][0] + connection[r * size])
        total = last[rid][0] + connection[rid * size]
        path, end = [], n
        while end > 0:
            _, start, before, lid = states[end][rid]
            path.append((run[start:end], lid))
            end, rid = start, before
        path.reverse()
        return path, total

    def words_of(self, run: str, depth: int = 2):
        """The path of `split`, with a phrase the dictionary lists as one word (いい天気,
        明日の会議, ありがとうございます) taken apart again when its parts are words of two or
        more characters or function words and cost about the same."""
        result = []
        for surface, lid in self.split(run)[0]:
            parts = self.parts(surface, depth) if len(surface) >= 3 else None
            result.extend(parts or [(surface, lid)])
        return result

    def parts(self, surface: str, depth: int):
        if depth == 0:
            return None
        whole_cost = self.split(surface)[1]
        path, cost = self.split(surface, whole=False)
        if len(path) < 2 or cost - whole_cost > RESPLIT_MARGIN:
            return None
        for part, lid in path:
            if part not in self.words or (len(part) < 2 and self.ids[lid][0] not in ("助詞", "助動詞")):
                return None
        result = []
        for part, lid in path:
            inner = self.parts(part, depth - 1) if len(part) >= 3 else None
            result.extend(inner or [(part, lid)])
        return result

    def role(self, surface: str, lid: int) -> str:
        """content, joins (inflects or ends the unit before it), particle or prefix."""
        feature = self.ids[lid]
        first, second = feature[0], feature[1]
        if first == "助動詞":
            return "joins"
        if first in ("動詞", "形容詞"):
            return "joins" if second in ("非自立", "接尾") else "content"
        if first == "助詞":
            return "joins" if second == "接続助詞" and surface in JOINING_PARTICLES else "particle"
        if first == "名詞":
            return "joins" if second == "接尾" else "content"
        if first == "接頭詞":
            return "prefix"
        return "content"

    def units(self, run: str):
        """[(text, tail, is particle, head)]: tail is the inflecting ending of a content unit, or "";
        head is the word before the tail when it stands by itself (天気 of 天気です, not 食べ of
        食べました), or ""."""
        result, pending_prefix = [], False
        for surface, lid in self.words_of(run):
            role = self.role(surface, lid)
            if role == "joins" and result:
                text, tail, particle, head = result[-1]
                result[-1] = (text + surface, tail + surface if not particle else "", particle, head)
                pending_prefix = False
            elif role == "content" and pending_prefix:
                text, _, _, _ = result[-1]
                result[-1] = (text + surface, "", False, "" if self.ids[lid][0] in ("動詞", "形容詞") else "+")
                pending_prefix = False
            else:
                stands = role == "content" and self.ids[lid][0] not in ("動詞", "形容詞")
                result.append((surface, "", role == "particle", "+" if stands else ""))
                pending_prefix = role == "prefix"
        return [(text, tail, particle, text[:len(text) - len(tail)] if head and tail else "")
                for text, tail, particle, head in result]


def clauses(text: str):
    """Runs of Japanese characters; anything else (marks, spaces, letters, digits) ends a clause."""
    for run in JAPANESE_RUN.findall(unicodedata.normalize("NFKC", text)):
        if 2 <= len(run) <= 60:
            yield run


def source_lines(name: str, src: dict):
    path = ROOT / src["path"] if "path" in src else CACHE / "dl" / src["cache"]
    if src["kind"] == "tatoeba":
        with bz2.open(path, "rt", encoding="utf-8") as f:
            for line in f:
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 3 and parts[1] == "jpn":
                    yield parts[2]
    else:
        opener = lzma.open if path.suffix == ".xz" else open
        with opener(path, "rt", encoding="utf-8") as f:
            yield from f


_splitter = None


def _count_chunk(lines):
    global _splitter
    if _splitter is None:
        _splitter = Splitter()
    keys, pairs, tails, stats = collections.Counter(), collections.Counter(), collections.Counter(), collections.Counter()
    for line in lines:
        for run in clauses(line):
            stats["clauses"] += 1
            units = _splitter.units(run)
            if any(blocked_terms.ja_hit(unit[0]) for unit in units) or blocked_terms.ja_hit(run):
                stats["blocked clauses"] += 1
                continue
            stats["units"] += len(units)
            for text, tail, _, head in units:
                # 天気 → です, ありがとう → ございます: the ending is what follows the bare word.
                if head and len(tail) >= 2 and len(head) <= MAX_UNIT_CHARS:
                    keys[head] += 1
                    pairs[(head, tail)] += 1
            for index in range(1, len(units)):
                text, tail, particle, _ = units[index - 1]
                following = units[index][0]
                if len(following) > MAX_UNIT_CHARS:
                    continue
                if len(text) <= MAX_UNIT_CHARS:
                    keys[text] += 1
                    pairs[(text, following)] += 1
                    if particle:
                        tails[text] += 1
                if tail and tail != text:
                    keys[tail] += 1
                    pairs[(tail, following)] += 1
                    tails[tail] += 1
                if index >= 2:
                    joined = units[index - 2][0] + text
                    if len(joined) <= MAX_PAIR_KEY_CHARS:
                        keys[joined] += 1
                        pairs[(joined, following)] += 1
    return keys, pairs, tails, stats


def count(jobs: int):
    CACHE.mkdir(parents=True, exist_ok=True)
    context = multiprocessing.get_context("fork")
    for name, src in sources().items():
        lines = list(source_lines(name, src))
        chunks = [lines[i:i + 2000] for i in range(0, len(lines), 2000)]
        keys, pairs, tails, stats = collections.Counter(), collections.Counter(), collections.Counter(), collections.Counter()
        with context.Pool(jobs) as pool:
            for k, p, t, s in pool.imap_unordered(_count_chunk, chunks):
                keys.update(k); pairs.update(p); tails.update(t); stats.update(s)
        stats["lines"] = len(lines)
        with (CACHE / f"counts-{name}.tsv").open("w", encoding="utf-8") as out:
            out.write("# " + json.dumps(dict(stats), ensure_ascii=False) + "\n")
            for key in sorted(keys):
                out.write(f"K\t{key}\t{keys[key]}\t{tails.get(key, 0)}\n")
            for (key, following), value in sorted(pairs.items()):
                out.write(f"P\t{key}\t{following}\t{value}\n")
        print(f"{name}: {dict(stats)}, {len(keys)} keys, {len(pairs)} pairs")


# --------------------------------------------------------------------------- snapshot

def snapshot():
    keys, pairs, tails, report = collections.Counter(), collections.Counter(), collections.Counter(), {}
    for name, src in sources().items():
        weight = int(src.get("weight", 1))
        with (CACHE / f"counts-{name}.tsv").open(encoding="utf-8") as f:
            report[name] = json.loads(f.readline()[2:])
            report[name]["weight"] = weight
            for line in f:
                parts = line.rstrip("\n").split("\t")
                if parts[0] == "K":
                    keys[parts[1]] += weight * int(parts[2])
                    tails[parts[1]] += weight * int(parts[3])
                else:
                    pairs[(parts[1], parts[2])] += weight * int(parts[3])
    following = collections.defaultdict(list)
    for (key, text), value in pairs.items():
        total = keys[key]
        if total < MIN_KEY_COUNT or value < MIN_PAIR_COUNT or value < MIN_SHARE * total:
            continue
        if blocked_terms.ja_hit(key) or blocked_terms.ja_hit(text) or blocked_terms.ja_hit(key + text):
            continue
        if len(key.encode()) > MAX_KEY_BYTES or len(text.encode()) > MAX_TEXT_BYTES:
            continue
        if key.startswith(CORPUS_NAMES) or text.startswith(CORPUS_NAMES) or text == "ー":
            continue
        following[key].append((value, text))
    rows = []
    for key in sorted(following, key=lambda k: k.encode()):
        best = sorted(following[key], key=lambda item: (-item[0], item[1]))[:MAX_NEXT_PER_KEY]
        tail = 2 * tails[key] >= keys[key]
        rows.append((key, FLAG_TAIL if tail else 0, keys[key], best))
    with lzma.open(SNAPSHOT, "wt", encoding="utf-8", preset=9) as out:
        out.write("# mimo-jp-next, generated by scripts/mimo_jp_next.py snapshot; do not edit. "
                  "key|flags|count|next:count ...\n")
        for key, flags, total, best in rows:
            out.write("|".join([key, str(flags), str(total)] + [f"{text}:{value}" for value, text in best]) + "\n")
    summary = {"keys": len(rows), "next": sum(len(row[3]) for row in rows),
               "tail_keys": sum(1 for row in rows if row[1] & FLAG_TAIL), "sources": report,
               "rules": {"min_key_count": MIN_KEY_COUNT, "min_pair_count": MIN_PAIR_COUNT, "min_share": MIN_SHARE,
                         "max_next_per_key": MAX_NEXT_PER_KEY, "max_unit_chars": MAX_UNIT_CHARS,
                         "max_pair_key_chars": MAX_PAIR_KEY_CHARS},
               "snapshot_sha256": sha256(SNAPSHOT)}
    (DATA / "counts.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(json.dumps({k: summary[k] for k in ("keys", "next", "tail_keys")}))


def read_snapshot(path: Path = SNAPSHOT):
    rows = []
    with lzma.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            parts = line.rstrip("\n").split("|")
            best = []
            for item in parts[3:]:
                text, value = item.rsplit(":", 1)
                best.append((int(value), text))
            rows.append((parts[0], int(parts[1]), int(parts[2]), best))
    return rows


# --------------------------------------------------------------------------- image

def _align(length):
    return (length + 7) & ~7


def write_image(rows) -> bytes:
    """rows: [(key, flags, count, [(count, text)])]. Layout: crates/meowpuut-core/src/japanese_next.rs."""
    rows = sorted(rows, key=lambda row: row[0].encode())
    blob, offsets = bytearray(), {}

    def intern(text):
        data = text.encode()
        if data not in offsets:
            offsets[data] = len(blob)
            blob.extend(data)
        return offsets[data], len(data)

    keys, nexts = bytearray(), bytearray()
    count = 0
    for key, flags, _, best in rows:
        best = best[:MAX_NEXT_PER_KEY]
        offset, length = intern(key)
        keys += struct.pack("<IHBBI", offset, length, len(best), flags, count)
        for value, text in best:
            text_offset, text_length = intern(text)
            nexts += struct.pack("<IHH", text_offset, text_length, max(1, min(65535, value)))
        count += len(best)
    keys_offset = HEADER_LEN
    nexts_offset = _align(keys_offset + len(keys))
    blob_offset = _align(nexts_offset + len(nexts))
    file_len = _align(blob_offset + len(blob))
    image = bytearray(file_len)
    image[:HEADER_LEN] = MAGIC + struct.pack("<9I", VERSION, len(rows), count, keys_offset, nexts_offset,
                                             blob_offset, len(blob), file_len, 0)
    image[keys_offset:keys_offset + len(keys)] = keys
    image[nexts_offset:nexts_offset + len(nexts)] = nexts
    image[blob_offset:blob_offset + len(blob)] = blob
    return bytes(image)


def read_image(data: bytes):
    if data[:12] != MAGIC:
        raise ValueError("not a MIMO-JN-1 image")
    version, key_count, _, keys_offset, nexts_offset, blob_offset, blob_len, file_len, _ = struct.unpack_from("<9I", data, 12)
    if version != VERSION or file_len != len(data):
        raise ValueError("wrong version or length")
    blob = data[blob_offset:blob_offset + blob_len]
    rows = []
    for index in range(key_count):
        offset, length, n, flags, first = struct.unpack_from("<IHBBI", data, keys_offset + KEY_RECORD_LEN * index)
        best = []
        for slot in range(first, first + n):
            text_offset, text_length, value = struct.unpack_from("<IHH", data, nexts_offset + NEXT_RECORD_LEN * slot)
            best.append((value, blob[text_offset:text_offset + text_length].decode()))
        rows.append((blob[offset:offset + length].decode(), flags, 0, best))
    return rows


def pack(out: Path):
    rows = []
    for key, flags, total, best in read_snapshot():
        if blocked_terms.ja_hit(key):
            continue
        best = [(value, text) for value, text in best if not blocked_terms.ja_hit(text) and not blocked_terms.ja_hit(key + text)]
        if best:
            rows.append((key, flags, total, best))
    image = write_image(rows)
    kept = read_image(image)
    if [(k, f, b) for k, f, _, b in kept] != [(k, f, [(max(1, min(65535, v)), t) for v, t in b[:MAX_NEXT_PER_KEY]])
                                              for k, f, _, b in sorted(rows, key=lambda row: row[0].encode())]:
        raise SystemExit("mimo_jp_next.bin does not read back as written")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(image)
    print(f"{out.name}: {len(rows)} keys, {sum(len(r[3]) for r in rows)} next words, {len(image)} bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fetch")
    counting = commands.add_parser("count")
    counting.add_argument("--jobs", type=int, default=8)
    commands.add_parser("snapshot")
    packing = commands.add_parser("pack")
    packing.add_argument("out", type=Path)
    showing = commands.add_parser("show")
    showing.add_argument("keys", nargs="+")
    args = parser.parse_args()
    if args.command == "fetch":
        fetch()
    elif args.command == "count":
        count(args.jobs)
    elif args.command == "snapshot":
        snapshot()
    elif args.command == "pack":
        pack(args.out)
    else:
        table = {row[0]: row for row in read_snapshot()}
        for key in args.keys:
            row = table.get(key)
            print(key, "->", "(none)" if row is None else
                  f"flags {row[1]} count {row[2]}: " + " ".join(f"{text}:{value}" for value, text in row[3]))


if __name__ == "__main__":
    sys.exit(main())
