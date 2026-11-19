#!/usr/bin/env python3
"""Pack Stage-2 bridge-trigram and skip-1 tables into `MIMO-TRI-1\0` binary images.

Subcommands:
  pack     Reads `trigram_bridge.tsv.xz` + `skip1_bigram.tsv.xz`, filters
           against `Lexicon` IDs and `blocked_terms`, and writes `mimo_trigrams.bin`.
  reweight Patches header `tri_weight_q8` / `skip_weight_q8` in place for tuning sweeps.
"""

from __future__ import annotations

import argparse
from collections import Counter
import lzma
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402
import mimo_pairs  # noqa: E402

MAGIC = b"MIMO-TRI-1\0\0"
VERSION = 1
HEADER_FMT = "<12sIIIQIIii16s"
MAX_BRIDGES = 64
MAX_WORD_ID_COUNT = 1 << 20
WEIGHT_OFFSET = 40
WEIGHT_FMT = "<ii"


def _open_tsv(path: Path):
    if path.suffix == ".xz":
        return lzma.open(path, "rt", encoding="utf-8")
    return open(path, encoding="utf-8")


def _word_id(token: str, ids: dict[str, int], allowed: dict[str, bool]) -> int | None:
    if token == "<s>":
        return mimo_pairs.START
    if token == "</s>":
        return mimo_pairs.END
    wid = ids.get(token)
    if wid is None or wid >= MAX_WORD_ID_COUNT:
        return None
    ok = allowed.get(token)
    if ok is None:
        ok = not (blocked_terms.zh_hit(token) or blocked_terms.drop_hit(token))
        allowed[token] = ok
    return wid if ok else None


def _pack_tri_key(w1: int, b_idx: int, w3: int) -> int:
    return (w1 << 28) | (b_idx << 20) | w3


def _pack_skip_key(w1: int, w3: int) -> int:
    return (w1 << 20) | w3


def pack_trigrams_image(
    lex: mimo_pairs.Lexicon,
    tri_path: Path,
    skip_path: Path,
    tri_weight: float,
    skip_weight: float,
    max_trigrams: int,
    max_skip1: int,
) -> tuple[bytes, dict[str, int]]:
    """Build a verified `MIMO-TRI-1` binary payload from compressed Stage-2 TSVs."""
    n_words = len(lex.forms)
    if n_words == 0 or n_words > MAX_WORD_ID_COUNT:
        raise ValueError(f"word count out of 20-bit range: {n_words}")

    ids = lex.ids
    allowed: dict[str, bool] = {}

    raw_tris: list[tuple[int, int, int, int, int]] = []
    bridge_freq: Counter[int] = Counter()
    with _open_tsv(tri_path) as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 4:
                continue
            w1 = _word_id(parts[0], ids, allowed)
            b_id = _word_id(parts[1], ids, allowed)
            w3 = _word_id(parts[2], ids, allowed)
            if w1 is None or b_id is None or w3 is None:
                continue
            score_q6 = max(-2048, min(2047, int(parts[3])))
            cnt = int(float(parts[4])) if len(parts) >= 5 else score_q6
            raw_tris.append((w1, b_id, w3, score_q6, cnt))
            bridge_freq[b_id] += cnt

    bridge_ids = [b_id for b_id, _ in bridge_freq.most_common(MAX_BRIDGES)]
    bridge_slot = {b_id: idx for idx, b_id in enumerate(bridge_ids)}

    # Keep highest-evidence unique keys up to cap, then sort ascending by packed key.
    tri_best: dict[int, tuple[int, int]] = {}
    for w1, b_id, w3, score_q6, cnt in raw_tris:
        b_idx = bridge_slot.get(b_id)
        if b_idx is None:
            continue
        key = _pack_tri_key(w1, b_idx, w3)
        prev = tri_best.get(key)
        if prev is None or (cnt, score_q6) > (prev[1], prev[0]):
            tri_best[key] = (score_q6, cnt)

    if len(tri_best) > max_trigrams:
        top_keys = sorted(tri_best.items(), key=lambda kv: (-kv[1][1], -kv[1][0], kv[0]))[:max_trigrams]
        tri_best = dict(top_keys)

    tri_u64 = sorted(((key << 12) | (vals[0] & 0xFFF)) for key, vals in tri_best.items())

    skip_best: dict[int, tuple[int, int]] = {}
    with _open_tsv(skip_path) as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 3:
                continue
            w1 = _word_id(parts[0], ids, allowed)
            w3 = _word_id(parts[1], ids, allowed)
            if w1 is None or w3 is None:
                continue
            score_q6 = max(-2048, min(2047, int(parts[2])))
            cnt = int(float(parts[3])) if len(parts) >= 4 else score_q6
            key = _pack_skip_key(w1, w3)
            prev = skip_best.get(key)
            if prev is None or (cnt, score_q6) > (prev[1], prev[0]):
                skip_best[key] = (score_q6, cnt)

    if len(skip_best) > max_skip1:
        top_keys = sorted(skip_best.items(), key=lambda kv: (-kv[1][1], -kv[1][0], kv[0]))[:max_skip1]
        skip_best = dict(top_keys)

    skip_u64 = sorted(((key << 12) | (vals[0] & 0xFFF)) for key, vals in skip_best.items())

    tri_q8 = int(round(tri_weight * 256.0))
    skip_q8 = int(round(skip_weight * 256.0))
    header = struct.pack(
        HEADER_FMT, MAGIC, VERSION, n_words, len(bridge_ids), lex.fingerprint,
        len(tri_u64), len(skip_u64), tri_q8, skip_q8, b"\x00" * 16,
    )
    padded_bridges = bridge_ids + [0] * (MAX_BRIDGES - len(bridge_ids))
    bridges_bin = struct.pack(f"<{MAX_BRIDGES}I", *padded_bridges)
    body = bytearray(len(header) + len(bridges_bin) + (len(tri_u64) + len(skip_u64)) * 8)
    offset = 0
    body[offset : offset + len(header)] = header
    offset += len(header)
    body[offset : offset + len(bridges_bin)] = bridges_bin
    offset += len(bridges_bin)
    for entry in tri_u64:
        struct.pack_into("<Q", body, offset, entry)
        offset += 8
    for entry in skip_u64:
        struct.pack_into("<Q", body, offset, entry)
        offset += 8

    stats = {
        "words": n_words,
        "bridges": len(bridge_ids),
        "tri_entries": len(tri_u64),
        "skip_entries": len(skip_u64),
        "tri_q8": tri_q8,
        "skip_q8": skip_q8,
        "bytes": len(body),
    }
    return bytes(body), stats


def cmd_pack(args):
    lex_dir = Path(args.lexicon_dir)
    lex = mimo_pairs.Lexicon(mimo_pairs.read_rows(lex_dir, "pinyin"), mimo_pairs.read_rows(lex_dir, "zhuyin"))
    payload, stats = pack_trigrams_image(
        lex,
        Path(args.trigrams),
        Path(args.skip1),
        args.tri_weight,
        args.skip_weight,
        args.max_trigrams,
        args.max_skip1,
    )
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(payload)
    print(f"wrote {out}: {stats}")


def reweight_image(path: Path, tri_weight: float, skip_weight: float) -> tuple[int, int]:
    """Patch Q8 header weights of an existing `MIMO-TRI-1` image in place."""
    data = bytearray(path.read_bytes())
    if len(data) < 64 + MAX_BRIDGES * 4 or data[:12] != MAGIC:
        raise SystemExit(f"{path}: not a valid MIMO-TRI-1 image")
    tri_q8 = int(round(tri_weight * 256.0))
    skip_q8 = int(round(skip_weight * 256.0))
    struct.pack_into(WEIGHT_FMT, data, WEIGHT_OFFSET, tri_q8, skip_q8)
    path.write_bytes(data)
    return tri_q8, skip_q8


def cmd_reweight(args):
    tri_q8, skip_q8 = reweight_image(Path(args.image), args.tri_weight, args.skip_weight)
    print(f"reweighted {args.image}: tri_q8={tri_q8}, skip_q8={skip_q8}")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    pk = sub.add_parser("pack")
    pk.add_argument("--lexicon-dir", required=True)
    pk.add_argument("--trigrams", required=True)
    pk.add_argument("--skip1", required=True)
    pk.add_argument("--out", required=True)
    pk.add_argument("--tri-weight", type=float, default=0.25)
    pk.add_argument("--skip-weight", type=float, default=0.15)
    pk.add_argument("--max-trigrams", type=int, default=450_000)
    pk.add_argument("--max-skip1", type=int, default=350_000)
    pk.set_defaults(fn=cmd_pack)

    rw = sub.add_parser("reweight")
    rw.add_argument("--image", required=True)
    rw.add_argument("--tri-weight", type=float, required=True)
    rw.add_argument("--skip-weight", type=float, required=True)
    rw.set_defaults(fn=cmd_reweight)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
