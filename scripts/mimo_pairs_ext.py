#!/usr/bin/env python3
"""Helper extensions for scripts/mimo_pairs.py: TSV eval exclusions and per-source weight scaling."""
import array
from collections import defaultdict
from pathlib import Path


def tsv_texts(path: Path) -> list[str]:
    """Extracts column-1 eval/heldout sentences from a tab-separated eval file."""
    texts = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) >= 2 and cols[1].strip():
            texts.append(cols[1].strip())
    return texts


def merge_weighted_sources(items: list[tuple[float, array.array, array.array, array.array]], n_forms: int):
    """Sums per-source `(weight_scale, uni, codes, counts)` into rounded integer `(uni, codes, counts)`."""
    uni_f = [0.0] * n_forms
    pair_acc: dict[int, float] = defaultdict(float)
    for scale, u, codes, cnts in items:
        if scale <= 0.0:
            continue
        for i in range(n_forms):
            if u[i]:
                uni_f[i] += scale * u[i]
        for code, c in zip(codes, cnts):
            pair_acc[code] += scale * c
    uni = [round(v) for v in uni_f]
    sorted_codes = array.array("Q")
    sorted_counts = array.array("Q")
    for code in sorted(pair_acc):
        val = round(pair_acc[code])
        if val > 0:
            sorted_codes.append(code)
            sorted_counts.append(val)
    return uni, (sorted_codes, sorted_counts)
