"""Writer and reader of the Japanese lexicon image MIMO-JP-1 (mimo_jp.bin).

Layout (little-endian, offsets from the start of the file, sections 8-byte aligned); the Rust
reader is crates/meowpuut-core/src/japanese.rs:

  header, 64 bytes: magic "MIMO-JP-1\\0\\0\\0", then u32 version, key_count, posting_count,
    word_count, reviewed_count, keys_offset, postings_offset, words_offset, reviewed_offset,
    blob_offset, blob_len, file_len, chinese_build
  key record, 12 bytes, sorted by key bytes: u32 key_offset, u16 key_len, u16 posting_count,
    u32 first_posting
  posting record, 8 bytes, grouped by key: u32 word, i16 weight, u8 kind, u8 flags
  word record, 8 bytes: u32 surface_offset, u16 surface_len, u8 class, u8 flags
  reviewed record, 16 bytes, sorted by reading: u32 reading_offset, u16 reading_len,
    u16 text_len, u32 text_offset, i16 weight, u16 flags
  blob: the key strings back to back in key order, then every other string once
"""
import struct

MAGIC = b"MIMO-JP-1\0\0\0"
VERSION = 1
HEADER_LEN = 64

KIND_READING, KIND_VARIANT, KIND_ALIAS_KANA, KIND_ALIAS_KANJI, KIND_AUTHORED = range(5)
FLAG_PINYIN_OK, FLAG_REVIEWED, FLAG_FORM = 1, 2, 4
REVIEWED_POOL = 1

# Index in this list is the class byte of a word record. `adj?` was added by the pipeline
# (tier B adjectives and irregular ones); it comes last so the other indexes stay as designed.
CLASSES = ["n", "pn", "v1", "v5k", "v5g", "v5s", "v5t", "v5n", "v5b", "v5m", "v5r", "v5w", "vs",
           "vk", "v?", "adj-i", "adj-na", "adv", "adn", "conj", "int", "place", "adj?"]

MAX_KEY_BYTES = 96
MAX_SURFACE_BYTES = 128
MAX_POSTINGS_PER_KEY = 64
MAX_REVIEWED = 4096


def _align(length):
    return (length + 7) // 8 * 8


def posting_order(posting, words):
    """Order inside a key: reviewed first, then weight descending, then surface by code point."""
    return (not posting["flags"] & FLAG_REVIEWED, -posting["weight"], words[posting["word"]]["surface"])


def write_image(words, postings, reviewed, chinese_build=0):
    """Return the image bytes.

    words: list of {surface, class, flags}; postings: list of {key, word, weight, kind, flags};
    reviewed: list of {reading, text, weight, flags}. Raises ValueError on a limit of the format.
    """
    by_key = {}
    for posting in postings:
        if not 1 <= posting["weight"] <= 10000 or not 0 <= posting["kind"] <= 4 or not 0 <= posting["flags"] <= 7:
            raise ValueError(f"posting out of range: {posting}")
        by_key.setdefault(posting["key"].encode(), []).append(posting)
    keys = sorted(by_key)
    for key in keys:
        group = by_key[key]
        if not 1 <= len(key) <= MAX_KEY_BYTES:
            raise ValueError(f"key length: {key.decode()!r}")
        if len(group) > MAX_POSTINGS_PER_KEY:
            raise ValueError(f"more than {MAX_POSTINGS_PER_KEY} postings under {key.decode()!r}")
        group.sort(key=lambda posting: posting_order(posting, words))
        surfaces = [words[posting["word"]]["surface"] for posting in group]
        if len(set(surfaces)) != len(surfaces):
            raise ValueError(f"surface twice under {key.decode()!r}: {surfaces}")
    rows = sorted(reviewed, key=lambda row: row["reading"].encode())
    if len(rows) > MAX_REVIEWED:
        raise ValueError("too many reviewed rows")

    blob = bytearray()
    key_records = bytearray()
    posting_records = bytearray()
    first = 0
    for key in keys:
        group = by_key[key]
        key_records += struct.pack("<IHHI", len(blob), len(key), len(group), first)
        blob += key
        for posting in group:
            posting_records += struct.pack("<IhBB", posting["word"], posting["weight"], posting["kind"], posting["flags"])
        first += len(group)

    offsets = {}

    def place(text):
        data = text.encode()
        if data not in offsets:
            offsets[data] = len(blob)
            blob.extend(data)
        return offsets[data]

    word_records = bytearray()
    for word in words:
        data = word["surface"].encode()
        if not 1 <= len(data) <= MAX_SURFACE_BYTES:
            raise ValueError(f"surface length: {word['surface']!r}")
        word_records += struct.pack("<IHBB", place(word["surface"]), len(data), word["class"], word["flags"])
    reviewed_records = bytearray()
    for row in rows:
        reading_at, text_at = place(row["reading"]), place(row["text"])
        reviewed_records += struct.pack("<IHHIhH", reading_at, len(row["reading"].encode()),
                                        len(row["text"].encode()), text_at, row["weight"], row["flags"])

    keys_at = HEADER_LEN
    postings_at = _align(keys_at + len(key_records))
    words_at = _align(postings_at + len(posting_records))
    reviewed_at = _align(words_at + len(word_records))
    blob_at = _align(reviewed_at + len(reviewed_records))
    file_len = blob_at + len(blob)
    if file_len >= 1 << 31:
        raise ValueError("image too large")
    out = bytearray(MAGIC)
    out += struct.pack("<13I", VERSION, len(keys), len(postings), len(words), len(rows), keys_at, postings_at,
                       words_at, reviewed_at, blob_at, len(blob), file_len, chinese_build)
    for start, section in ((keys_at, key_records), (postings_at, posting_records), (words_at, word_records),
                           (reviewed_at, reviewed_records), (blob_at, blob)):
        out += bytes(start - len(out))
        out += section
    assert len(out) == file_len
    return bytes(out)


def read_image(data):
    """Parse a whole image into the structures write_image takes; raises ValueError when damaged."""
    if len(data) < HEADER_LEN or data[:12] != MAGIC:
        raise ValueError("not a MIMO-JP-1 image")
    (version, key_count, posting_count, word_count, reviewed_count, keys_at, postings_at, words_at,
     reviewed_at, blob_at, blob_len, file_len, chinese_build) = struct.unpack_from("<13I", data, 12)
    if version != VERSION or file_len != len(data) or blob_at + blob_len != file_len:
        raise ValueError("header does not match the file")
    blob = data[blob_at:blob_at + blob_len]

    def text(offset, length):
        if offset + length > len(blob):
            raise ValueError("string outside the blob")
        return blob[offset:offset + length].decode()

    words = []
    for index in range(word_count):
        offset, length, cls, flags = struct.unpack_from("<IHBB", data, words_at + 8 * index)
        words.append({"surface": text(offset, length), "class": cls, "flags": flags})
    raw = [struct.unpack_from("<IhBB", data, postings_at + 8 * index) for index in range(posting_count)]
    postings, keys = [], []
    for index in range(key_count):
        offset, length, count, first = struct.unpack_from("<IHHI", data, keys_at + 12 * index)
        key = text(offset, length)
        keys.append((key, first, count))
        for word, weight, kind, flags in raw[first:first + count]:
            postings.append({"key": key, "word": word, "weight": weight, "kind": kind, "flags": flags})
    reviewed = []
    for index in range(reviewed_count):
        r_at, r_len, t_len, t_at, weight, flags = struct.unpack_from("<IHHIhH", data, reviewed_at + 16 * index)
        reviewed.append({"reading": text(r_at, r_len), "text": text(t_at, t_len), "weight": weight, "flags": flags})
    sections = {"header": HEADER_LEN, "keys": 12 * key_count, "postings": 8 * posting_count,
                "words": 8 * word_count, "reviewed": 16 * reviewed_count, "blob": blob_len,
                "key strings": sum(len(key.encode()) for key, _, _ in keys)}
    return {"words": words, "postings": postings, "reviewed": reviewed, "keys": keys,
            "chinese_build": chinese_build, "sections": sections}


def chinese_build_of(data):
    """The chinese_build field for a mimo_base.bin: its sha256's first four bytes, read little-endian."""
    import hashlib
    return int.from_bytes(hashlib.sha256(data).digest()[:4], "little")
