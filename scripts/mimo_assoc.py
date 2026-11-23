#!/usr/bin/env python3
"""Builds the script-aware cross-word association table `mimo_assoc.bin` (`MIMO-ASC-1`).

Scans clean project-owned `DataSources/mimo-everyday/*.txt.xz` (and optional clean shards)
using lexicon Viterbi segmentation, keeping separate Traditional (`tc`) and Simplified (`sc`)
cross-word transitions:
  - 1-word context: `w1 -> w2` (e.g. `幫我 -> 看一下`, `今天 -> 天氣`)
  - 2-word context: `w1+w2 -> w3` (e.g. `今天天氣 -> 真好`, `我想去 -> 吃飯`)

Strictly excludes blocked terms (`scripts/blocked_terms.py`) and `Tests/eval/*.tsv` 5-grams.

Usage:
  python3 scripts/mimo_assoc.py build --lexicon-dir .build/lexicon-v4 --out .build/lexicon-v4/mimo_assoc.bin
"""
import argparse
import heapq
import json
import lzma
import math
import re
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402
import colab_corpus_mix as mix  # noqa: E402

MAGIC = b"MIMO-ASC-1\x00\x00"
VERSION = 1
HEADER_BYTES = 64
HAN_RUN = re.compile(r"[㐀-䶿一-鿿]+")
MAX_KEY_CHARS = 4
MAX_VAL_CHARS = 4


def load_script_maps(lex_dir: Path) -> tuple[dict[str, float], dict[int, str], set[str], set[str], int]:
    """Loads lexicon word log-probs + 1-char Trad->Simp map + distinct TC-only / SC-only char sets."""
    words_sc: dict[str, int] = {}
    words_tc: dict[str, int] = {}
    t2s_best: dict[str, tuple[str, int]] = {}
    tc_only_chars: set[str] = set()
    sc_only_chars: set[str] = set()
    with open(lex_dir / "native-pinyin.tsv", "r", encoding="utf-8") as f:
        next(f)
        for line in f:
            if line == "# context\n":
                break
            p = line.rstrip("\n").split("\t")
            if len(p) >= 5 and p[4].lstrip("-").isdigit():
                hant, hans, w = p[2], p[3], int(p[4])
                if HAN_RUN.fullmatch(hans) and w > words_sc.get(hans, -9999):
                    words_sc[hans] = w
                if HAN_RUN.fullmatch(hant) and w > words_tc.get(hant, -9999):
                    words_tc[hant] = w
                if len(hant) == 1 and len(hans) == 1:
                    if t2s_best.get(hant, ("", -9999))[1] < w:
                        t2s_best[hant] = (hans, w)
                    if hant != hans:
                        tc_only_chars.add(hant)
                        sc_only_chars.add(hans)
    # Remove any char that appears in both sets so classification is unambiguous.
    both = tc_only_chars & sc_only_chars
    tc_only_chars -= both
    sc_only_chars -= both
    t2s = {ord(k): v for k, (v, _) in t2s_best.items() if k != v}
    merged: dict[str, int] = dict(words_sc)
    for w, sc in words_tc.items():
        if sc > merged.get(w, -9999):
            merged[w] = sc
    logp = {w: (sc / 700.0) - 19.0 for w, sc in merged.items()}
    return logp, t2s, tc_only_chars, sc_only_chars, min(max((len(w) for w in merged), default=2), 6)


def classify_line_script(line: str, tc_only: set[str], sc_only: set[str]) -> str:
    """Returns 'tc', 'sc', or 'both' (for script-neutral sentences)."""
    tc_hits = sum(1 for ch in line if ch in tc_only)
    sc_hits = sum(1 for ch in line if ch in sc_only)
    if tc_hits > sc_hits:
        return "tc"
    if sc_hits > tc_hits:
        return "sc"
    return "both"


def extract_transitions(
    corpus_dirs: list[Path],
    word_logp: dict[str, float],
    t2s: dict[int, str],
    tc_only: set[str],
    sc_only: set[str],
    max_len: int,
):
    eval_grams = mix.load_eval_grams(ROOT / "Tests/eval", t2s)
    blocked_re = blocked_terms.counted()
    files = mix.iter_source_files(corpus_dirs)

    uni = {"tc": Counter(), "sc": Counter()}
    ctx = {"tc": Counter(), "sc": Counter()}
    trans = {"tc": Counter(), "sc": Counter()}
    lines_seen = {"tc": 0, "sc": 0, "both": 0}

    for fp in files:
        wgt = max(1, round(mix.weight_for_file(fp.stem.lower()) * 10))
        opener = lzma.open if fp.suffix == ".xz" else open
        with opener(fp, "rt", encoding="utf-8", errors="ignore") as f:
            for raw in f:
                s = raw.strip()
                if not s or blocked_re.search(s) or mix.has_eval_overlap(s, eval_grams, t2s):
                    continue
                script = classify_line_script(s, tc_only, sc_only)
                lines_seen[script] += 1
                targets = ("tc", "sc") if script == "both" else (script,)
                for run in HAN_RUN.findall(s):
                    if len(run) < 2:
                        continue
                    toks = mix.segment_han_dp(run, word_logp, max_len)
                    if len(toks) < 2:
                        continue
                    for tg in targets:
                        for i, tok in enumerate(toks):
                            uni[tg][tok] += wgt
                            if i + 1 < len(toks):
                                nxt = toks[i + 1]
                                if len(tok) <= MAX_KEY_CHARS and len(nxt) <= MAX_VAL_CHARS and tok != nxt:
                                    ctx[tg][tok] += wgt
                                    trans[tg][(tok, nxt)] += wgt
                            if i + 2 < len(toks):
                                k2 = tok + toks[i + 1]
                                nxt2 = toks[i + 2]
                                if len(k2) <= MAX_KEY_CHARS and len(nxt2) <= MAX_VAL_CHARS and toks[i + 1] != nxt2:
                                    ctx[tg][k2] += wgt
                                    trans[tg][(k2, nxt2)] += wgt
    return uni, ctx, trans, lines_seen


def select_top_rows(
    uni: Counter,
    ctx: Counter,
    trans: Counter,
    max_keys: int,
    max_per_key: int,
    min_count: int,
) -> list[tuple[str, list[tuple[str, int]]]]:
    tot_uni = max(1, sum(uni.values()))
    grouped: dict[str, list[tuple[float, str, int]]] = defaultdict(list)
    for (k, v), cnt in trans.items():
        if cnt < min_count:
            continue
        c_k = ctx[k]
        u_v = uni[v]
        if c_k == 0 or u_v == 0:
            continue
        # Blend log conditional probability and smoothed PMI so both common
        # natural followers ("看一下") and tight collocations ("真好") rank high.
        p_cond = cnt / c_k
        pmi = math.log(p_cond / max(u_v / tot_uni, 1e-12))
        if pmi < 0.25 and p_cond < 0.04:
            continue
        raw_score = 0.55 * math.log1p(cnt) + 0.45 * max(0.0, pmi) + 2.5 * p_cond
        grouped[k].append((raw_score, v, cnt))

    # Keep the highest-evidence context keys.
    ranked_keys = heapq.nlargest(max_keys, grouped.keys(), key=lambda k: (ctx[k], len(grouped[k])))
    rows: list[tuple[str, list[tuple[str, int]]]] = []
    for k in sorted(ranked_keys, key=lambda x: x.encode("utf-8")):
        cands = grouped[k]
        cands.sort(key=lambda item: (-item[0], -item[2], item[1]))
        top = cands[:max_per_key]
        if not top:
            continue
        # Quantize scores to u16 (1000..65000) preserving cross-key strength.
        out_vals = []
        for raw_sc, val, _ in top:
            q = int(max(100, min(65000, round(raw_sc * 3500.0))))
            out_vals.append((val, q))
        rows.append((k, out_vals))
    return rows


def pack_assoc(tc_rows: list[tuple[str, list[tuple[str, int]]]], sc_rows: list[tuple[str, list[tuple[str, int]]]]) -> bytes:
    pool = bytearray()
    intern_map: dict[str, tuple[int, int]] = {}

    def intern(s: str) -> tuple[int, int]:
        hit = intern_map.get(s)
        if hit is not None:
            return hit
        b = s.encode("utf-8")
        off = len(pool)
        pool.extend(b)
        pair = (off, len(b))
        intern_map[s] = pair
        return pair

    def encode_table(rows):
        k_off, k_len, e_off = [], [], [0]
        v_off, v_len, scores = [], [], []
        for k, vals in rows:
            ko, kl = intern(k)
            k_off.append(ko)
            k_len.append(kl)
            for v, sc in vals:
                vo, vl = intern(v)
                v_off.append(vo)
                v_len.append(vl)
                scores.append(sc)
            e_off.append(len(v_off))
        buf = bytearray()
        buf.extend(struct.pack(f"<{len(k_off)}I", *k_off) if k_off else b"")
        buf.extend(struct.pack(f"<{len(k_len)}H", *k_len) if k_len else b"")
        buf.extend(struct.pack(f"<{len(e_off)}I", *e_off))
        buf.extend(struct.pack(f"<{len(v_off)}I", *v_off) if v_off else b"")
        buf.extend(struct.pack(f"<{len(v_len)}H", *v_len) if v_len else b"")
        buf.extend(struct.pack(f"<{len(scores)}H", *scores) if scores else b"")
        return len(k_off), len(v_off), bytes(buf)

    tc_k, tc_e, tc_bytes = encode_table(tc_rows)
    sc_k, sc_e, sc_bytes = encode_table(sc_rows)
    header = struct.pack("<12s6I", MAGIC, VERSION, tc_k, tc_e, sc_k, sc_e, len(pool)) + b"\x00" * (HEADER_BYTES - 36)
    return header + tc_bytes + sc_bytes + bytes(pool)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    pb = sub.add_parser("build")
    pb.add_argument("--lexicon-dir", required=True)
    pb.add_argument("--corpus-dirs", nargs="+", default=[str(ROOT / "DataSources/mimo-everyday")])
    pb.add_argument("--out", required=True)
    pb.add_argument("--max-keys-per-script", type=int, default=45000)
    pb.add_argument("--max-per-key", type=int, default=8)
    pb.add_argument("--min-count", type=int, default=150)
    args = ap.parse_args()

    lex_dir = Path(args.lexicon_dir)
    word_logp, t2s, tc_only, sc_only, max_len = load_script_maps(lex_dir)
    uni, ctx, trans, lines_seen = extract_transitions(
        [Path(d) for d in args.corpus_dirs], word_logp, t2s, tc_only, sc_only, max_len
    )
    tc_rows = select_top_rows(uni["tc"], ctx["tc"], trans["tc"], args.max_keys_per_script, args.max_per_key, args.min_count)
    sc_rows = select_top_rows(uni["sc"], ctx["sc"], trans["sc"], args.max_keys_per_script, args.max_per_key, args.min_count)
    blob = pack_assoc(tc_rows, sc_rows)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(blob)
    summary = {
        "out": str(out),
        "bytes": len(blob),
        "lines_seen": lines_seen,
        "tc_keys": len(tc_rows),
        "tc_edges": sum(len(v) for _, v in tc_rows),
        "sc_keys": len(sc_rows),
        "sc_edges": sum(len(v) for _, v in sc_rows),
    }
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
