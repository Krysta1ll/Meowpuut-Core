#!/usr/bin/env python3
"""Audit and ingest Batch-5 hydrogen-low general corpus & multi-domain/ACG/meme lexicon.

Subcommands:
  audit   Inspect every worker shard for naturalness, template collapse, duplicates, and leaks.
  ingest  Clean, deduplicate, compress into DataSources/mimo-everyday/ & DataSources/mimo-domains/,
          and update SOURCE.json manifests in mimo-everyday and mimo-pairs.
"""
import argparse
import hashlib
import json
import lzma
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

STAGE = ROOT / ".build/hydrogen-corpus-v5"
EV_DIR = ROOT / "DataSources/mimo-everyday"
DOM_DIR = ROOT / "DataSources/mimo-domains"
PAIRS_SRC = ROOT / "DataSources/mimo-pairs/SOURCE.json"

HAN = re.compile(r"[㐀-䶿一-鿿]")
HAN_WORD = re.compile(r"^[㐀-䶿一-鿿]{2,8}$")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")
CLEAN_CELL = re.compile(r"[\s0-9.\-•*]+")

BUCKET_MAP = {
    "chat-tw-1": ("chat", True), "chat-tw-2": ("chat", True), "chat-tw-3": ("chat", True),
    "chat-cn-1": ("chat", False), "chat-cn-2": ("chat", False), "chat-cn-3": ("chat", False),
    "acg-meme-1": ("chat", True), "acg-meme-2": ("chat", False),
    "written-tw-1": ("written", True), "written-tw-2": ("written", True), "written-tw-3": ("written", True),
    "written-cn-1": ("written", False), "written-cn-2": ("written", False), "written-cn-3": ("written", False),
    "colloc-tw-1": ("written", True), "colloc-tw-2": ("written", True),
    "colloc-cn-1": ("written", False), "colloc-cn-2": ("written", False),
    "mixed-cs-1": ("mixed", True), "mixed-cs-2": ("mixed", False),
}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_t2s() -> dict[int, str]:
    for cand in (
        ROOT / ".build/lexicon-v3/native-pinyin.tsv",
        ROOT / ".build/lexicon-v2/native-pinyin.tsv",
        ROOT / ".build/lexicon/native-pinyin.tsv",
    ):
        if cand.exists():
            best: dict[str, tuple[str, int]] = {}
            with open(cand, "r", encoding="utf-8") as f:
                next(f, None)
                for ln in f:
                    if ln == "# context\n":
                        break
                    p = ln.rstrip("\n").split("\t")
                    if len(p) >= 5 and len(p[2]) == 1 and len(p[3]) == 1 and p[4].lstrip("-").isdigit():
                        w = int(p[4])
                        if best.get(p[2], ("", -9999))[1] < w:
                            best[p[2]] = (p[3], w)
            return {ord(k): v for k, (v, _) in best.items() if k != v}
    return {}


def load_eval_grams(t2s: dict[int, str]) -> set[str]:
    grams: set[str] = set()
    eval_dir = ROOT / "Tests/eval"
    for p in sorted(eval_dir.glob("*.tsv")) if eval_dir.exists() else []:
        for ln in p.read_text(encoding="utf-8").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                txt = cols[1].strip()
                for form in (txt, txt.translate(t2s)):
                    clean = "".join(HAN.findall(form))
                    for i in range(max(0, len(clean) - 4)):
                        grams.add(clean[i : i + 5])
    return grams


def load_existing_corpus() -> set[str]:
    seen: set[str] = set()
    for xp in sorted(EV_DIR.glob("*.txt.xz")):
        if xp.name in ("hydrogen-chat-5.txt.xz", "hydrogen-written-5.txt.xz", "hydrogen-mixed-5.txt.xz"):
            continue
        with lzma.open(xp, "rt", encoding="utf-8") as f:
            seen.update(f.read().splitlines())
    return seen


def load_existing_terms() -> set[tuple[str, str]]:
    seen: set[tuple[str, str]] = set()
    for fp in sorted(DOM_DIR.glob("*.tsv")):
        if fp.name == "hydrogen-v5-terms.tsv":
            continue
        for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                seen.add((cols[0].strip(), cols[1].strip()))
    return seen


def template_metrics(lines: list[str]) -> dict:
    if not lines:
        return {"n": 0, "top2": 0.0, "top4_suf": 0.0, "trigram_div": 0.0, "max4g_share": 0.0, "ok": False}
    p2, s4, g3, g4_doc = Counter(), Counter(), Counter(), Counter()
    total_g3 = 0
    for ln in lines:
        han = "".join(HAN.findall(ln))
        if len(han) >= 2:
            p2[han[:2]] += 1
        if len(han) >= 4:
            s4[han[-4:]] += 1
        for i in range(max(0, len(han) - 2)):
            g3[han[i : i + 3]] += 1
            total_g3 += 1
        seen4 = {han[i : i + 4] for i in range(max(0, len(han) - 3))}
        for g in seen4:
            g4_doc[g] += 1
    n = len(lines)
    top2 = (p2.most_common(1)[0][1] / n) if p2 else 0.0
    top4_suf = (s4.most_common(1)[0][1] / n) if s4 else 0.0
    trigram_div = (len(g3) / total_g3) if total_g3 else 0.0
    max4g_share = (g4_doc.most_common(1)[0][1] / n) if g4_doc else 0.0
    # Natural text has high trigram diversity (>0.50 for 1k lines) and low 4-gram collision (<10%).
    ok = (n >= 500) and (top4_suf <= 0.08) and (trigram_div >= 0.42) and (max4g_share <= 0.14)
    return {
        "n": n,
        "top2": round(top2, 4),
        "top4_suf": round(top4_suf, 4),
        "trigram_div": round(trigram_div, 4),
        "max4g_share": round(max4g_share, 4),
        "ok": ok,
    }


def cmd_audit(_args):
    raw_root = STAGE / "raw"
    print("=== CORPUS SHARDS ===")
    total_lines = 0
    for name in sorted(BUCKET_MAP):
        d = raw_root / name
        lines = []
        files = sorted(d.glob("*.txt")) if d.exists() else []
        for fp in files:
            lines.extend(ln.strip() for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines() if ln.strip())
        m = template_metrics(lines)
        total_lines += m["n"]
        flag = "OK" if m["ok"] else "CHECK"
        print(
            f"{name:14s} files={len(files):2d} lines={m['n']:5d} "
            f"top2={m['top2']:.1%} suf4={m['top4_suf']:.1%} g3div={m['trigram_div']:.2f} max4g={m['max4g_share']:.1%} [{flag}]"
        )
    print(f"TOTAL CORPUS RAW LINES: {total_lines:,}\n")

    terms_root = STAGE / "terms"
    print("=== LEXICON TERM SHARDS ===")
    total_terms = 0
    for d in sorted(terms_root.glob("terms-*")) if terms_root.exists() else []:
        files = sorted(d.glob("*.tsv"))
        cnt = sum(
            1
            for fp in files
            for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines()
            if ln.strip() and not ln.startswith("#")
        )
        total_terms += cnt
        print(f"{d.name:14s} files={len(files):2d} terms={cnt:5d}")
    print(f"TOTAL RAW TERMS: {total_terms:,}")


def cmd_ingest(args):
    t2s = load_t2s()
    eval_grams = load_eval_grams(t2s)
    seen = load_existing_corpus()
    blocked_re = blocked_terms.counted()

    buckets = {"chat": [], "written": [], "mixed": []}
    tw_counts = {"chat": 0, "written": 0, "mixed": 0}
    stats = Counter()
    skip_set = set(args.skip or [])

    for name, (bucket, is_tw) in sorted(BUCKET_MAP.items()):
        if name in skip_set:
            continue
        d = STAGE / "raw" / name
        if not d.exists():
            continue
        for fp in sorted(d.glob("*.txt")):
            for raw in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
                if not raw.strip():
                    continue
                stats["raw"] += 1
                s = LIST_PREFIX.sub("", raw).strip().strip("「」\"'")
                if not (6 <= len(s) <= 40) or BROKEN.search(s) or len(HAN.findall(s)) < 4:
                    stats["bad_format"] += 1
                    continue
                if s in seen:
                    stats["duplicate"] += 1
                    continue
                han_only = "".join(HAN.findall(s))
                if blocked_re.search(s):
                    stats["blocked"] += 1
                    continue
                if any(
                    x[i : i + 5] in eval_grams
                    for x in (han_only, han_only.translate(t2s))
                    for i in range(max(0, len(x) - 4))
                ):
                    stats["eval_overlap"] += 1
                    continue
                seen.add(s)
                buckets[bucket].append(s)
                if is_tw:
                    tw_counts[bucket] += 1

    # Write compressed corpus files
    out_files = {
        "hydrogen-chat-5.txt.xz": buckets["chat"],
        "hydrogen-written-5.txt.xz": buckets["written"],
        "hydrogen-mixed-5.txt.xz": buckets["mixed"],
    }
    for fname, lines in out_files.items():
        with lzma.open(EV_DIR / fname, "wt", encoding="utf-8", preset=6) as f:
            f.write("\n".join(lines) + ("\n" if lines else ""))

    # Update DataSources/mimo-everyday/SOURCE.json
    ev_meta_path = EV_DIR / "SOURCE.json"
    ev_meta = json.loads(ev_meta_path.read_text(encoding="utf-8"))
    for fname in out_files:
        ev_meta.setdefault("files", {})[fname] = sha256(EV_DIR / fname)
    ev_meta["batch_5"] = {
        "written_date": "2026-11-23",
        "author": "Meowpuut project (Gemini 4 Hydrogen Low swarm, hand-authored per micro-topic)",
        "hydrogen-chat-5": len(buckets["chat"]),
        "hydrogen-written-5": len(buckets["written"]),
        "hydrogen-mixed-5": len(buckets["mixed"]),
        "tw_counts": tw_counts,
        "stats": dict(stats),
    }
    ev_meta_path.write_text(json.dumps(ev_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    # Register in DataSources/mimo-pairs/SOURCE.json
    pairs_meta = json.loads(PAIRS_SRC.read_text(encoding="utf-8"))
    srcs = pairs_meta.setdefault("sources", {})
    srcs["hydrogen-chat-5"] = {
        "path": "DataSources/mimo-everyday/hydrogen-chat-5.txt.xz",
        "group": "spoken",
        "kind": "lines",
        "weight_scale": 1.5,
        "version": "written 2026-11-23 for this project (DataSources/mimo-everyday/SOURCE.json)",
        "license": "written for this project",
    }
    srcs["hydrogen-written-5"] = {
        "path": "DataSources/mimo-everyday/hydrogen-written-5.txt.xz",
        "group": "written",
        "kind": "lines",
        "weight_scale": 2.0,
        "version": "written 2026-11-23 for this project (DataSources/mimo-everyday/SOURCE.json)",
        "license": "written for this project",
    }
    srcs["hydrogen-mixed-5"] = {
        "path": "DataSources/mimo-everyday/hydrogen-mixed-5.txt.xz",
        "group": "written",
        "kind": "lines",
        "weight_scale": 1.5,
        "version": "written 2026-11-23 for this project (DataSources/mimo-everyday/SOURCE.json)",
        "license": "written for this project",
    }
    PAIRS_SRC.write_text(json.dumps(pairs_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    # Ingest domain/ACG/meme terms
    seen_terms = load_existing_terms()
    term_rows = []
    term_stats = Counter()
    field_counts = Counter()
    terms_root = STAGE / "terms"
    if terms_root.exists():
        for fp in sorted(terms_root.glob("terms-*/*.tsv")):
            for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
                s_ln = ln.strip()
                if not s_ln or s_ln.startswith("#"):
                    continue
                term_stats["raw"] += 1
                parts = [p.strip() for p in s_ln.split("\t")]
                if len(parts) < 2:
                    term_stats["bad_cols"] += 1
                    continue
                t, s = CLEAN_CELL.sub("", parts[0]), CLEAN_CELL.sub("", parts[1])
                field = parts[2].strip() if len(parts) >= 3 and parts[2].strip() else fp.parent.name
                if len(t) != len(s) or not HAN_WORD.fullmatch(t) or not HAN_WORD.fullmatch(s):
                    term_stats["bad_han"] += 1
                    continue
                if blocked_terms.zh_hit(s) or blocked_terms.drop_hit(s) or blocked_terms.zh_hit(t) or blocked_terms.drop_hit(t):
                    term_stats["blocked"] += 1
                    continue
                if (t, s) in seen_terms:
                    term_stats["duplicate"] += 1
                    continue
                seen_terms.add((t, s))
                term_rows.append((t, s, field))
                field_counts[field] += 1

    term_rows.sort()
    out_terms = DOM_DIR / "hydrogen-v5-terms.tsv"
    lines = ["# Project-owned Gemini 4 Hydrogen Low multi-domain, ACG & internet-meme terms (Traditional, Simplified, Field)"]
    lines.extend(f"{t}\t{s}\t{f}" for t, s, f in term_rows)
    out_terms.write_text("\n".join(lines) + "\n", encoding="utf-8")

    # Update DataSources/mimo-domains/SOURCE.json
    dom_meta_path = DOM_DIR / "SOURCE.json"
    if dom_meta_path.exists():
        dom_meta = json.loads(dom_meta_path.read_text(encoding="utf-8"))
        dom_meta.setdefault("files", {})["hydrogen-v5-terms.tsv"] = sha256(out_terms)
        dom_meta["hydrogen_v5_term_count"] = len(term_rows)
        dom_meta_path.write_text(json.dumps(dom_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(
        f"INGESTED CORPUS: chat={len(buckets['chat']):,} written={len(buckets['written']):,} "
        f"mixed={len(buckets['mixed']):,} total={sum(len(v) for v in buckets.values()):,} stats={dict(stats)}"
    )
    print(f"INGESTED TERMS: kept={len(term_rows):,} stats={dict(term_stats)} top_fields={field_counts.most_common(12)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("audit")
    pi = sub.add_parser("ingest")
    pi.add_argument("--skip", nargs="*", default=[])
    args = ap.parse_args()
    (cmd_audit if args.cmd == "audit" else cmd_ingest)(args)


if __name__ == "__main__":
    main()
