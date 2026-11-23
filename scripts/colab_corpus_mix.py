#!/usr/bin/env python3
"""Clean Open-Source + Project-Owned Context Table Builder (Bigram / Bridged Trigram / Skip-1).

Only uses project-authored `DataSources/mimo-everyday` and `DataSources/mimo-pairs` sources
(CC-BY-2.0-FR, CC0-1.0, Apache-2.0, CC-BY-4.0). Zero web crawls and zero CC BY-SA text.

Subcommands:
  export-clean-lines    Exports holdout-safe `.txt.xz` lines from approved mimo-pairs sources.
  build-context-tables  Builds rebalanced Bigram + Function-Word Trigram + Skip-1 Bigram tables.
"""
import argparse
import heapq
import json
import lzma
import math
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402
import mimo_pairs  # noqa: E402

CLAUSE_SPLIT = re.compile(r"[。．！？!?；;\n\r]+")
HAN_RUN = re.compile(r"[㐀-䶿一-鿿]+")
BRIDGE_WORDS = frozenset("的了在一个片把很不是给会要想到和与还都就也")

SOURCE_WEIGHTS = {
    "hydrogen-chat": 10.0,
    "hydrogen-written": 2.0,
    "hydrogen-mixed": 1.5,
    "modern-chat-3": 10.0,
    "colloc-chat-4": 10.0,
    "everyday": 10.0,
    "chat-2": 10.0,
    "modern-domains-3": 2.0,
    "colloc-domains-4": 2.0,
    "written-2": 1.5,
    "mixed-2": 1.5,
    "commonvoice": 3.5,
    "massive": 1.5,
    "tatoeba": 3.5,
    "kdconv": 0.35,
    "crosswoz": 0.25,
}


def load_eval_grams(eval_dir: Path, t2s: dict[int, str] | None = None) -> set[str]:
    grams = set()
    for p in sorted(eval_dir.glob("*.tsv")):
        for line in p.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) >= 2:
                s = cols[1].strip()
                forms = (s, s.translate(t2s)) if t2s else (s,)
                for f in forms:
                    clean = "".join(HAN_RUN.findall(f))
                    for i in range(max(0, len(clean) - 4)):
                        grams.add(clean[i : i + 5])
    return grams


def has_eval_overlap(text: str, grams: set[str], t2s: dict[int, str] | None = None) -> bool:
    clean = "".join(HAN_RUN.findall(text))
    forms = (clean, clean.translate(t2s)) if t2s else (clean,)
    return any(x[i : i + 5] in grams for x in forms for i in range(max(0, len(x) - 4)))


def load_lexicon_maps(lexicon_dir: Path) -> tuple[dict[str, float], dict[int, str], int]:
    """Loads Simplified word log-scores + Traditional->Simplified 1-char map from native-pinyin.tsv."""
    words: dict[str, int] = {}
    t2s_best: dict[str, tuple[str, int]] = {}
    path = lexicon_dir / "native-pinyin.tsv"
    if not path.exists():
        raise SystemExit(f"missing lexicon TSV: {path}")
    with open(path, "r", encoding="utf-8") as f:
        next(f)
        for line in f:
            if line == "# context\n":
                break
            p = line.rstrip("\n").split("\t")
            if len(p) >= 5 and p[4].lstrip("-").isdigit():
                hant, hans, w = p[2], p[3], int(p[4])
                if HAN_RUN.fullmatch(hans) and w > words.get(hans, -9999):
                    words[hans] = w
                if len(hant) == 1 and len(hans) == 1 and t2s_best.get(hant, ("", -9999))[1] < w:
                    t2s_best[hant] = (hans, w)
    longest = max((len(w) for w in words), default=2)
    t2s = {ord(k): v for k, (v, _) in t2s_best.items() if k != v}
    log_w = {w: (wgt / 700.0) - 19.0 for w, wgt in words.items()}
    return log_w, t2s, min(longest, 8)


def segment_han_dp(run: str, word_logp: dict[str, float], max_len: int) -> list[str]:
    """Viterbi segmentation over lexicon Simplified words (fast, deterministic)."""
    n = len(run)
    best = [-1e12] * (n + 1)
    back = [0] * (n + 1)
    best[0] = 0.0
    for i in range(n):
        sc = best[i]
        for j in range(i + 1, min(n, i + max_len) + 1):
            tok = run[i:j]
            lw = word_logp.get(tok)
            if lw is None and j == i + 1:
                lw = -18.0
            if lw is not None and sc + lw > best[j]:
                best[j] = sc + lw
                back[j] = i
    out = []
    j = n
    while j > 0:
        i = back[j]
        out.append(run[i:j])
        j = i
    out.reverse()
    return out


def extract_clean_clauses(text: str, blocked_re, eval_grams: set[str], t2s: dict[int, str] | None = None) -> list[str]:
    clauses = []
    for seg in CLAUSE_SPLIT.split(text):
        s = seg.strip()
        if 6 <= len(s) <= 44 and sum(len(x) for x in HAN_RUN.findall(s)) >= 5:
            if not blocked_re.search(s) and not has_eval_overlap(s, eval_grams, t2s):
                clauses.append(s)
    return clauses


def cmd_export_clean_lines(args):
    """Exports approved non-repo sources in .build/pair-corpus/dl to holdout-safe `.txt.xz` shards."""
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    _, t2s, _ = load_lexicon_maps(Path(args.lexicon_dir))
    eval_grams = load_eval_grams(ROOT / "Tests/eval", t2s)
    blocked_re = blocked_terms.counted()
    manifest = {}
    for name, src in mimo_pairs.sources().items():
        if "path" in src:
            continue  # Already under DataSources/mimo-everyday
        dl_path = mimo_pairs.CACHE / "dl" / src["cache"]
        if not dl_path.exists():
            continue
        bucket = name.split("-")[0]
        kept = []
        for raw in mimo_pairs.source_texts(src):
            kept.extend(extract_clean_clauses(raw, blocked_re, eval_grams, t2s))
        out_path = out_dir / f"pair_{name}.txt.xz"
        with lzma.open(out_path, "wt", encoding="utf-8", preset=6) as f:
            f.write("\n".join(kept) + ("\n" if kept else ""))
        manifest[name] = {"bucket": bucket, "clauses": len(kept), "license": src["license"]}
    (out_dir / "clean_export_manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"export-clean-lines: exported {len(manifest)} sources -> {out_dir}")


def weight_for_file(stem: str) -> float:
    for key, w in SOURCE_WEIGHTS.items():
        if key in stem:
            return w
    return 1.0


def iter_source_files(dirs: list[Path]) -> list[Path]:
    files = []
    for d in dirs:
        if d.exists():
            files.extend(sorted(d.glob("*.txt.xz")))
            files.extend(sorted(d.glob("*.txt")))
    return files


def count_corpus_ngrams(files: list[Path], word_logp: dict[str, float], t2s: dict[int, str], max_len: int, eval_grams: set[str], blocked_re):
    uni: Counter[str] = Counter()
    ctx: Counter[str] = Counter()
    bi: Counter[tuple[str, str]] = Counter()
    tri: Counter[tuple[str, str, str]] = Counter()
    skip1: Counter[tuple[str, str]] = Counter()
    src_stats = {}

    for fp in files:
        wgt = max(1, round(weight_for_file(fp.stem.lower()) * 10))
        opener = lzma.open if fp.suffix == ".xz" else open
        lines_seen = 0
        with opener(fp, "rt", encoding="utf-8", errors="ignore") as f:
            for line in f:
                s = line.strip()
                if not s or blocked_re.search(s) or has_eval_overlap(s, eval_grams, t2s):
                    continue
                lines_seen += 1
                simp = s.translate(t2s)
                for run in HAN_RUN.findall(simp):
                    toks = segment_han_dp(run, word_logp, max_len)
                    if not toks:
                        continue
                    seq = ["<s>", *toks, "</s>"]
                    for i, tok in enumerate(seq):
                        uni[tok] += wgt
                        if i + 1 < len(seq):
                            ctx[tok] += wgt
                            bi[(tok, seq[i + 1])] += wgt
                        if 0 < i < len(seq) - 2 and seq[i + 1] in BRIDGE_WORDS and len(seq[i + 2]) >= 2:
                            tri[(tok, seq[i + 1], seq[i + 2])] += wgt
                        if 0 < i < len(seq) - 2 and seq[i + 2] not in ("<s>", "</s>") and (len(tok) >= 2 or len(seq[i + 2]) >= 2):
                            skip1[(tok, seq[i + 2])] += wgt
        src_stats[fp.name] = {"lines": lines_seen, "weight_x10": wgt}
    return uni, ctx, bi, tri, skip1, src_stats


def cmd_build_context_tables(args):
    word_logp, t2s, max_len = load_lexicon_maps(Path(args.lexicon_dir))
    eval_grams = load_eval_grams(ROOT / "Tests/eval", t2s)
    blocked_re = blocked_terms.counted()
    files = iter_source_files([Path(d) for d in args.corpus_dirs])
    if not files:
        raise SystemExit(f"no .txt/.txt.xz corpus files found in {args.corpus_dirs}")

    uni, ctx, bi, tri, skip1, src_stats = count_corpus_ngrams(files, word_logp, t2s, max_len, eval_grams, blocked_re)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    tot_uni = max(1, sum(uni.values()))

    grouped: dict[str, list[tuple[str, int]]] = defaultdict(list)
    for (left, right), cnt in bi.most_common(args.top_pairs):
        grouped[left].append((right, cnt))
    with lzma.open(out_dir / "rebalanced_pairs.txt.xz", "wt", encoding="utf-8", preset=6) as f:
        f.write("# rebalanced counts from colab_corpus_mix.py\n")
        for w, cnt in sorted(uni.items()):
            f.write(f"U\t{w}\t{cnt}\t{ctx[w]}\t{cnt}\n")
        for left in sorted(grouped):
            f.write("P\t" + left + "\t" + "\t".join(f"{r} {c}" for r, c in sorted(grouped[left])) + "\n")

    def pmi_score(pair_cnt: int, left_cnt: int, right_cnt: int, min_cnt: int) -> int | None:
        if pair_cnt < min_cnt or left_cnt == 0 or right_cnt == 0:
            return None
        pmi = math.log((pair_cnt / left_cnt) / max(right_cnt / tot_uni, 1e-12))
        val = round(max(-255.0, min(255.0, 0.8 * pmi * 64.0)))
        return val if val > 8 else None

    top_tri = heapq.nlargest(args.top_trigrams, tri.items(), key=lambda kv: kv[1])
    tri_written = 0
    with lzma.open(out_dir / "trigram_bridge.tsv.xz", "wt", encoding="utf-8", preset=6) as f:
        for (w1, bridge, w3), cnt in sorted(top_tri, key=lambda kv: kv[0]):
            sc = pmi_score(cnt, bi[(w1, bridge)], uni[w3], args.min_trigram_count)
            if sc is not None:
                f.write(f"{w1}\t{bridge}\t{w3}\t{sc}\t{cnt}\n")
                tri_written += 1

    top_skip = heapq.nlargest(args.top_skip1, skip1.items(), key=lambda kv: kv[1])
    skip_written = 0
    with lzma.open(out_dir / "skip1_bigram.tsv.xz", "wt", encoding="utf-8", preset=6) as f:
        for (w1, w3), cnt in sorted(top_skip, key=lambda kv: kv[0]):
            sc = pmi_score(cnt, uni[w1], uni[w3], args.min_trigram_count)
            if sc is not None:
                f.write(f"{w1}\t{w3}\t{sc}\t{cnt}\n")
                skip_written += 1

    summary = {
        "files_processed": len(files), "unique_unigrams": len(uni), "unique_bigrams": len(bi),
        "trigrams_kept": tri_written, "skip1_kept": skip_written, "sources": src_stats,
    }
    (out_dir / "context_tables_summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False, indent=2))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    pe = sub.add_parser("export-clean-lines")
    pe.add_argument("--lexicon-dir", required=True)
    pe.add_argument("--out-dir", default=str(ROOT / ".build/colab-corpus/shards"))

    pb = sub.add_parser("build-context-tables")
    pb.add_argument("--lexicon-dir", required=True)
    pb.add_argument("--corpus-dirs", nargs="+", required=True)
    pb.add_argument("--out-dir", default=str(ROOT / ".build/colab-corpus/tables"))
    pb.add_argument("--top-pairs", type=int, default=2_000_000)
    pb.add_argument("--top-trigrams", type=int, default=350_000)
    pb.add_argument("--top-skip1", type=int, default=250_000)
    pb.add_argument("--min-trigram-count", type=int, default=3)
    args = ap.parse_args()
    {"export-clean-lines": cmd_export_clean_lines, "build-context-tables": cmd_build_context_tables}[args.cmd](args)


if __name__ == "__main__":
    main()
