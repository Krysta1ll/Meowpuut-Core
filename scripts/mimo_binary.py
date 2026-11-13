"""Binary serializer for MIMO-BIN-1 and MIMO-BASE-1 (mimo_base.bin) SoA lexicons.

mimo_base.bin version 2 adds one MIMO-IDX-1 section per mode with the lookup
structures the engine used to build on the heap at load (two hash indexes,
two character conversion tables, the initials lists), so the engine can read
the whole file in place. The layout is described in
crates/meowpuut-core/src/bin_format.rs; Lexicon::pack_base writes the same
bytes, and crates/meowpuut-core/tests/in_place_base.rs checks that.
"""
import array
import pathlib
import re
import struct
import sys

MIMO_BIN_MAGIC = b"MIMO-BIN-1\x00\x00"
MIMO_BASE_MAGIC = b"MIMO-BASE-1\x00"
MIMO_IDX_MAGIC = b"MIMO-IDX-1\x00\x00"
MIMO_BASE_VERSION = 2

FLAG_FULLY_SPELLED = 1 << 0
FLAG_SINGLE_HAN_HANT = 1 << 1
FLAG_SINGLE_HAN_HANS = 1 << 2
FLAG_VERIFIED_SPELLING = 1 << 3

_VOWELS = set(b"aeiouv")
_SYLLABLE_RE = re.compile(r"[^1-5]+[1-5]")


def _is_han_scalar(code: int) -> bool:
    return (
        0x3400 <= code <= 0x9FFF
        or 0xF900 <= code <= 0xFAFF
        or 0x20000 <= code <= 0x323AF
    )


def _is_single_han(text: str, utf8_len: int) -> bool:
    if utf8_len > 4 or len(text) != 1:
        return False
    return _is_han_scalar(ord(text[0]))


def _is_fully_spelled_reading(reading_bytes: bytes) -> bool:
    vowel = False
    for b in reading_bytes:
        if 49 <= b <= 53:  # '1'..'5'
            if not vowel:
                return False
            vowel = False
        elif b in _VOWELS:
            vowel = True
    return True


def compile_mimo_bin(
    pinyin: bool,
    sorted_entries: list[tuple[tuple[str, str, str, str], int]],
    sorted_context: list[tuple[str, int]],
) -> bytes:
    """Serialize sorted lexicon rows and context phrases into a MIMO-BIN-1 image."""
    storage_parts: list[bytes] = []
    storage_len = 0

    reading_offset = array.array("I")
    reading_len = array.array("H")
    hant_offset = array.array("I")
    hant_len = array.array("H")
    hans_offset = array.array("I")
    hans_len = array.array("H")
    weight_col = array.array("h")
    flags_col = bytearray()

    key_offset = array.array("I")
    key_len = array.array("H")
    word_start = array.array("I")
    word_end = array.array("I")

    phrase_offset = array.array("I")
    phrase_len = array.array("H")
    frequency_col = array.array("h")

    verified_single_syllables: set[str] = set()
    if pinyin:
        for (_, reading, hant, _), _ in sorted_entries:
            if len(hant) == 1 and len(hant.encode("utf-8")) <= 4 and _is_han_scalar(ord(hant)):
                verified_single_syllables.add(reading)

    last_bare: str | None = None
    longest = 0

    for word_idx, ((bare, reading, hant, hans), weight) in enumerate(sorted_entries):
        if last_bare != bare:
            bare_b = bare.encode("utf-8")
            b_len = len(bare_b)
            key_offset.append(storage_len)
            key_len.append(b_len)
            word_start.append(word_idx)
            word_end.append(word_idx + 1)
            storage_parts.append(bare_b)
            storage_len += b_len
            if b_len > longest:
                longest = b_len
            last_bare = bare
        else:
            word_end[-1] = word_idx + 1

        read_b = reading.encode("utf-8")
        r_off = storage_len
        r_len = len(read_b)
        storage_parts.append(read_b)
        storage_len += r_len

        hant_b = hant.encode("utf-8")
        t_off = storage_len
        t_len = len(hant_b)
        storage_parts.append(hant_b)
        storage_len += t_len

        if hans == hant:
            s_off = t_off
            s_len = t_len
        else:
            hans_b = hans.encode("utf-8")
            s_off = storage_len
            s_len = len(hans_b)
            storage_parts.append(hans_b)
            storage_len += s_len

        f = 0
        if _is_fully_spelled_reading(read_b):
            f |= FLAG_FULLY_SPELLED
        if _is_single_han(hant, t_len):
            f |= FLAG_SINGLE_HAN_HANT
        if _is_single_han(hans, s_len):
            f |= FLAG_SINGLE_HAN_HANS
        if not pinyin:
            f |= FLAG_VERIFIED_SPELLING
        else:
            syls = _SYLLABLE_RE.findall(reading)
            if syls and sum(len(s) for s in syls) == len(reading) and all(
                s in verified_single_syllables for s in syls
            ):
                f |= FLAG_VERIFIED_SPELLING

        reading_offset.append(r_off)
        reading_len.append(r_len)
        hant_offset.append(t_off)
        hant_len.append(t_len)
        hans_offset.append(s_off)
        hans_len.append(s_len)
        clamped_w = max(-32768, min(32767, int(weight)))
        weight_col.append(clamped_w)
        flags_col.append(f)

    for phrase, freq in sorted_context:
        phrase_b = phrase.encode("utf-8")
        p_len = len(phrase_b)
        phrase_offset.append(storage_len)
        phrase_len.append(p_len)
        clamped_f = max(-32768, min(32767, int(freq)))
        frequency_col.append(clamped_f)
        storage_parts.append(phrase_b)
        storage_len += p_len

    if sys.byteorder != "little":
        for arr in (
            reading_offset, reading_len, hant_offset, hant_len, hans_offset, hans_len,
            weight_col, key_offset, key_len, word_start, word_end, phrase_offset,
            phrase_len, frequency_col,
        ):
            arr.byteswap()

    meta = (1 if pinyin else 0) | (min(255, longest) << 8)
    header = struct.pack(
        "<12sIIIII",
        MIMO_BIN_MAGIC,
        meta,
        storage_len,
        len(reading_offset),
        len(key_offset),
        len(phrase_offset),
    )
    storage_blob = b"".join(storage_parts)
    return b"".join([
        header,
        storage_blob,
        reading_offset.tobytes(),
        reading_len.tobytes(),
        hant_offset.tobytes(),
        hant_len.tobytes(),
        hans_offset.tobytes(),
        hans_len.tobytes(),
        weight_col.tobytes(),
        bytes(flags_col),
        key_offset.tobytes(),
        key_len.tobytes(),
        word_start.tobytes(),
        word_end.tobytes(),
        phrase_offset.tobytes(),
        phrase_len.tobytes(),
        frequency_col.tobytes(),
    ])


def pack_mimo_base_v1(pinyin_bin: bytes, zhuyin_bin: bytes) -> bytes:
    """Pack Pinyin and Zhuyin MIMO-BIN-1 blobs into a 64-byte aligned version 1 container."""
    py_offset = 64
    py_len = len(pinyin_bin)
    after_py = py_offset + py_len
    zy_offset = (after_py + 63) & ~63
    zy_len = len(zhuyin_bin)
    header = struct.pack(
        "<12sIIIII",
        MIMO_BASE_MAGIC,
        1,
        py_offset,
        py_len,
        zy_offset,
        zy_len,
    )
    pad1 = b"\x00" * (py_offset - len(header))
    pad2 = b"\x00" * (zy_offset - after_py)
    return b"".join([header, pad1, pinyin_bin, pad2, zhuyin_bin])


_M64 = (1 << 64) - 1
_K = 0x9E3779B97F4A7C15


def phrase_hash(data: bytes) -> int:
    """Same 64-bit hash as meowpuut_core::context_index::phrase_hash."""
    h = (len(data) * _K) & _M64
    for i in range(0, len(data), 8):
        word = int.from_bytes(data[i:i + 8], "little")
        h = ((h ^ word) * _K) & _M64
        h = ((h << 29) | (h >> 35)) & _M64
    h ^= h >> 32
    h = (h * 0xD6E8FEB86659FD93) & _M64
    return h ^ (h >> 32)


MAX_PROBE = 255  # context_index::MAX_PROBE; the reader refuses a longer probe distance


def _phrase_index(keys: list[bytes]) -> tuple[int, bytes, bytes, int]:
    """Open-addressing table like PhraseIndex::build: (capacity, slots, tags, longest probe distance)."""
    if not keys:
        return 0, b"", b"", 0
    cap = max(16, 1 << (len(keys) * 2 - 1).bit_length())
    mask = cap - 1
    slots = array.array("I", bytes(4 * cap))
    tags = array.array("H", bytes(2 * cap))
    probe = 0
    for row, key in enumerate(keys):
        h = phrase_hash(key)
        pos = h & mask
        distance = 0
        while slots[pos]:
            pos = (pos + 1) & mask
            distance += 1
        probe = max(probe, distance)
        slots[pos] = row + 1
        tags[pos] = h >> 48
    if probe > MAX_PROBE:
        raise ValueError(f"hash index probe distance {probe} exceeds {MAX_PROBE}")
    if sys.byteorder != "little":
        slots.byteswap()
        tags.byteswap()
    return cap, slots.tobytes(), tags.tobytes(), probe


def image_fingerprint(image: bytes) -> int:
    """Same as bin_format::image_fingerprint: hash of 64 evenly spaced 64-byte pieces."""
    if len(image) <= 4096:
        return phrase_hash(image)
    last = len(image) - 64
    return phrase_hash(b"".join(image[last * k // 63:last * k // 63 + 64] for k in range(64)))


def _u32s(values) -> bytes:
    arr = array.array("I", values)
    if sys.byteorder != "little":
        arr.byteswap()
    return arr.tobytes()


def _image_index(image: bytes) -> dict:
    """Lookup structures for one MIMO-BIN-1 image, as LexiconIndex::build makes them."""
    _, _, storage_len, n_words, n_buckets, n_context = struct.unpack_from("<12sIIIII", image)
    storage = image[32:32 + storage_len]
    pos = 32 + storage_len

    def col(code, count):
        nonlocal pos
        arr = array.array(code)
        arr.frombytes(image[pos:pos + arr.itemsize * count])
        if sys.byteorder != "little":
            arr.byteswap()
        pos += arr.itemsize * count
        return arr

    r_off, _r_len, t_off, t_len, s_off, s_len, weight = (
        col("I", n_words), col("H", n_words), col("I", n_words), col("H", n_words),
        col("I", n_words), col("H", n_words), col("h", n_words))
    flags = image[pos:pos + n_words]
    pos += n_words
    k_off, k_len = col("I", n_buckets), col("H", n_buckets)
    col("I", n_buckets), col("I", n_buckets)
    p_off, p_len = col("I", n_context), col("H", n_context)

    traditional: dict[bytes, tuple[int, int]] = {}
    simplified: dict[bytes, tuple[int, int]] = {}
    initials: list[list[int]] = [[] for _ in range(128)]
    both = FLAG_SINGLE_HAN_HANT | FLAG_SINGLE_HAN_HANS
    for i in range(n_words):
        w = weight[i]
        if flags[i] & both == both:
            hans = storage[s_off[i]:s_off[i] + s_len[i]]
            if 0 < len(hans) <= 4:
                old = traditional.get(hans)
                if old is None or old[1] < w:
                    traditional[hans] = (i, w)
            hant = storage[t_off[i]:t_off[i] + t_len[i]]
            if hant != hans and 0 < len(hant) <= 4:
                old = simplified.get(hant)
                if old is None or old[1] < w:
                    simplified[hant] = (i, w)
        if t_len[i] <= 4:
            first = storage[r_off[i]]
            if first < 128:
                initials[first].append(i)

    def char_table(table):
        rows = sorted((int.from_bytes(k.ljust(4, b"\x00"), "big"), i) for k, (i, _) in table.items())
        return len(rows), _u32s(k for k, _ in rows) + _u32s(i for _, i in rows)

    starts, flat = [0], []
    for items in initials:
        items.sort(key=lambda i: -weight[i])
        flat.extend(items[:64])
        starts.append(len(flat))
    return {
        "hans": [storage[s_off[i]:s_off[i] + s_len[i]] for i in range(n_words)],
        "bucket": _phrase_index([storage[k_off[i]:k_off[i] + k_len[i]] for i in range(n_buckets)]),
        "context_keys": [storage[p_off[i]:p_off[i] + p_len[i]] for i in range(n_context)],
        "traditional": char_table(traditional),
        "simplified": char_table(simplified),
        "initials": (len(flat), _u32s(starts) + _u32s(flat)),
    }


def _write_index(out: bytearray, image: bytes, index: dict, shared_context) -> tuple[int, int]:
    """Append a MIMO-IDX-1 section (96-byte header, see bin_format.rs); returns the (slots, tags) offsets of its context index."""
    start = len(out)
    out.extend(bytes(96))
    fields: list[int] = []

    def put(*parts: bytes) -> int:
        out.extend(bytes(-len(out) % 8))
        at = len(out)
        for part in parts:
            out.extend(part)
        return at

    cap, slots, tags, bucket_probe = index["bucket"]
    fields += [cap, put(slots), put(tags)]
    cap, slots, tags, context_probe = index["context"]
    context_at = shared_context or (put(slots), put(tags))
    fields += [cap, *context_at]
    for name in ("traditional", "simplified", "initials"):
        count, data = index[name]
        fields += [count, put(data)]
    ids_at = put(_u32s(index["word_ids"]))
    out[start:start + 96] = MIMO_IDX_MAGIC + struct.pack(
        "<13IQ2I2IQ", len(image), *fields, image_fingerprint(image), bucket_probe, context_probe,
        ids_at, index["word_count"], index["word_fingerprint"])
    return context_at


CLAUSE_START, CLAUSE_END = "<s>", "</s>"


def word_forms(hans_texts) -> list[str]:
    """Word id order (lexicon::WordIds): clause start, clause end, then the distinct simplified texts sorted."""
    texts = {t.decode("utf-8") if isinstance(t, bytes) else t for t in hans_texts}
    return [CLAUSE_START, CLAUSE_END] + sorted(texts)


def forms_fingerprint(forms: list[str]) -> int:
    """Hash of the sorted texts (markers left out) joined by newlines; a pair table carries the same number."""
    return phrase_hash("\n".join(forms[2:]).encode("utf-8"))


def pack_mimo_base(pinyin_bin: bytes, zhuyin_bin: bytes) -> bytes:
    """Pack both images into a version 2 MIMO-BASE-1 container with an index section per mode.

    The context index is written once when both images have the same context table.
    """
    py_index, zy_index = _image_index(pinyin_bin), _image_index(zhuyin_bin)
    forms = word_forms(py_index["hans"] + zy_index["hans"])
    ids = {text.encode("utf-8"): i for i, text in enumerate(forms)}
    for index in (py_index, zy_index):
        index["word_ids"] = [ids[text] for text in index["hans"]]
        index["word_count"] = len(forms)
        index["word_fingerprint"] = forms_fingerprint(forms)
    py_index["context"] = _phrase_index(py_index["context_keys"])
    same_context = zy_index["context_keys"] == py_index["context_keys"]
    zy_index["context"] = py_index["context"] if same_context else _phrase_index(zy_index["context_keys"])

    out = bytearray(pack_mimo_base_v1(pinyin_bin, zhuyin_bin))
    out[12:16] = struct.pack("<I", MIMO_BASE_VERSION)
    out.extend(bytes(-len(out) % 64))
    py_idx = len(out)
    context_at = _write_index(out, pinyin_bin, py_index, None)
    py_idx_len = len(out) - py_idx
    out.extend(bytes(-len(out) % 64))
    zy_idx = len(out)
    _write_index(out, zhuyin_bin, zy_index, context_at if same_context else None)
    zy_idx_len = len(out) - zy_idx
    out[32:48] = struct.pack("<4I", py_idx, py_idx_len, zy_idx, zy_idx_len)
    return bytes(out)


def write_mimo_binaries(
    output_dir: pathlib.Path,
    pinyin_entries: list[tuple[tuple[str, str, str, str], int]],
    zhuyin_entries: list[tuple[tuple[str, str, str, str], int]],
    sorted_context: list[tuple[str, int]],
) -> dict[str, int]:
    """Write the unified mimo_base.bin (MIMO-BASE-1 version 2) binary lexicon container."""
    py_bin = compile_mimo_bin(True, pinyin_entries, sorted_context)
    zy_bin = compile_mimo_bin(False, zhuyin_entries, sorted_context)
    base_bin = pack_mimo_base(py_bin, zy_bin)
    for redundant in ("native-pinyin.bin", "native-zhuyin.bin"):
        p = output_dir / redundant
        if p.exists():
            p.unlink()
    (output_dir / "mimo_base.bin").write_bytes(base_bin)
    return {
        "pinyin_bin_bytes": len(py_bin),
        "zhuyin_bin_bytes": len(zy_bin),
        "mimo_base_bin_bytes": len(base_bin),
    }
