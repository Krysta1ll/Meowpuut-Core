#!/usr/bin/env python3
"""Multi-batch audit and ingestion pipeline for hydrogen-low corpus & lexicon waves (Batch 6+).

Subcommands:
  audit   --batch N   Inspect every shard & file in .build/hydrogen-corpus-vN/
  ingest  --batch N   Filter, anti-template check, compress into DataSources/mimo-everyday/ &
                      DataSources/mimo-domains/, and update SOURCE.json manifests.
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
from check_hydrogen_shard import file_diversity, load_eval_grams, load_t2s  # noqa: E402

EV_DIR = ROOT / "DataSources/mimo-everyday"
DOM_DIR = ROOT / "DataSources/mimo-domains"
PAIRS_SRC = ROOT / "DataSources/mimo-pairs/SOURCE.json"

HAN = re.compile(r"[㐀-䶿一-鿿]")
HAN_WORD = re.compile(r"^[㐀-䶿一-鿿]{2,8}$")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")
CLEAN_CELL = re.compile(r"[\s0-9.\-•*]+")
BAD_FILLER_TAIL = set("呀啊呢吧啦嗎吗嘛哦喲哟")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def classify_worker(name: str) -> tuple[str, bool]:
    low = name.lower()
    if low.startswith("mixed"):
        bucket = "mixed"
    elif any(low.startswith(p) for p in ("written", "colloc", "domain", "know")):
        bucket = "written"
    else:
        bucket = "chat"
    is_tw = ("-tw" in low) or low.endswith("tw")
    return bucket, is_tw


def load_existing_corpus(batch: int) -> set[str]:
    skip = {f"hydrogen-chat-{batch}.txt.xz", f"hydrogen-written-{batch}.txt.xz", f"hydrogen-mixed-{batch}.txt.xz"}
    seen: set[str] = set()
    for xp in sorted(EV_DIR.glob("*.txt.xz")):
        if xp.name in skip:
            continue
        with lzma.open(xp, "rt", encoding="utf-8") as f:
            seen.update(f.read().splitlines())
    return seen


def load_existing_terms(batch: int) -> set[tuple[str, str]]:
    skip_name = f"hydrogen-v{batch}-terms.tsv"
    seen: set[tuple[str, str]] = set()
    for fp in sorted(DOM_DIR.glob("*.tsv")):
        if fp.name == skip_name:
            continue
        for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                seen.add((cols[0].strip(), cols[1].strip()))
    return seen


def cmd_audit(args):
    stage = ROOT / f".build/hydrogen-corpus-v{args.batch}"
    raw_root = stage / "raw"
    print(f"=== BATCH {args.batch} CORPUS SHARDS ===")
    total_lines = 0
    bad_files = []
    if raw_root.exists():
        for d in sorted(p for p in raw_root.iterdir() if p.is_dir()):
            lines, ok_files = [], 0
            files = sorted(d.glob("*.txt"))
            for fp in files:
                fl = [ln.strip() for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines() if ln.strip()]
                g3, suf4 = file_diversity(fl)
                if len(fl) >= 40 and (g3 < 0.50 or (suf4 > 0.12 and g3 < 0.65)):
                    bad_files.append((d.name, fp.name, len(fl), round(g3, 2), round(suf4, 3)))
                else:
                    ok_files += 1
                    lines.extend(fl)
            g3, suf4 = file_diversity(lines)
            total_lines += len(lines)
            print(f"{d.name:18s} files={ok_files:2d}/{len(files):2d} lines={len(lines):5d} g3div={g3:.2f} suf4={suf4:.1%}")
    print(f"TOTAL CLEAN CORPUS LINES: {total_lines:,} | BAD_TEMPLATE_FILES: {len(bad_files)}")
    for bf in bad_files:
        print("  BAD:", bf)

    terms_root = stage / "terms"
    if terms_root.exists():
        print(f"\n=== BATCH {args.batch} TERM SHARDS ===")
        tot_t = 0
        for d in sorted(p for p in terms_root.iterdir() if p.is_dir()):
            files = sorted(d.glob("*.tsv"))
            cnt = sum(
                1
                for fp in files
                for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines()
                if ln.strip() and not ln.startswith("#")
            )
            tot_t += cnt
            print(f"{d.name:18s} files={len(files):2d} terms={cnt:5d}")
        print(f"TOTAL RAW TERMS: {tot_t:,}")


def cmd_ingest(args):
    batch = args.batch
    stage = ROOT / f".build/hydrogen-corpus-v{batch}"
    t2s = load_t2s()
    eval_grams = load_eval_grams(t2s)
    seen = load_existing_corpus(batch)
    blocked_re = blocked_terms.counted()

    buckets = {"chat": [], "written": [], "mixed": []}
    tw_counts = {"chat": 0, "written": 0, "mixed": 0}
    stats = Counter()

    raw_root = stage / "raw"
    if raw_root.exists():
        for d in sorted(p for p in raw_root.iterdir() if p.is_dir()):
            bucket, is_tw = classify_worker(d.name)
            for fp in sorted(d.glob("*.txt")):
                fl_raw = [ln.strip() for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines() if ln.strip()]
                g3, suf4 = file_diversity(fl_raw)
                if len(fl_raw) >= 40 and (g3 < 0.50 or (suf4 > 0.12 and g3 < 0.65)):
                    stats["template_file_skipped"] += len(fl_raw)
                    continue
                for raw in fl_raw:
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

    out_files = {}
    for bucket, lines in buckets.items():
        if lines:
            fname = f"hydrogen-{bucket}-{batch}.txt.xz"
            out_files[fname] = (bucket, lines)
            with lzma.open(EV_DIR / fname, "wt", encoding="utf-8", preset=6) as f:
                f.write("\n".join(lines) + "\n")

    ev_meta_path = EV_DIR / "SOURCE.json"
    ev_meta = json.loads(ev_meta_path.read_text(encoding="utf-8"))
    batch_entry = {
        "written_date": "2026-11-23",
        "author": f"Meowpuut project (Gemini 4 Hydrogen Low swarm wave batch_{batch}, hand-authored)",
        "tw_counts": tw_counts,
        "stats": dict(stats),
    }
    for fname, (_, lines) in out_files.items():
        ev_meta.setdefault("files", {})[fname] = sha256(EV_DIR / fname)
        batch_entry[fname[:-7]] = len(lines)
    ev_meta[f"batch_{batch}"] = batch_entry
    ev_meta_path.write_text(json.dumps(ev_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    pairs_meta = json.loads(PAIRS_SRC.read_text(encoding="utf-8"))
    srcs = pairs_meta.setdefault("sources", {})
    for fname, (bucket, _) in out_files.items():
        key = fname[:-7]
        srcs[key] = {
            "path": f"DataSources/mimo-everyday/{fname}",
            "group": "spoken" if bucket == "chat" else "written",
            "kind": "lines",
            "weight_scale": 1.5 if bucket != "written" else 2.0,
            "version": "written 2026-11-23 for this project (DataSources/mimo-everyday/SOURCE.json)",
            "license": "written for this project",
        }
    PAIRS_SRC.write_text(json.dumps(pairs_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    # Terms ingestion (optional if terms dir exists)
    terms_root = stage / "terms"
    term_rows, term_stats, field_counts = [], Counter(), Counter()
    if terms_root.exists():
        seen_terms = load_existing_terms(batch)
        for fp in sorted(terms_root.glob("*/*.tsv")):
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
                if len(t) >= 4 and t[-1] in BAD_FILLER_TAIL:
                    term_stats["filler_tail"] += 1
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
        if term_rows:
            term_rows.sort()
            out_terms = DOM_DIR / f"hydrogen-v{batch}-terms.tsv"
            lines = [f"# Project-owned Gemini 4 Hydrogen Low batch-{batch} domain, ACG & meme terms (Traditional, Simplified, Field)"]
            lines.extend(f"{t}\t{s}\t{f}" for t, s, f in term_rows)
            out_terms.write_text("\n".join(lines) + "\n", encoding="utf-8")
            dom_meta_path = DOM_DIR / "SOURCE.json"
            if dom_meta_path.exists():
                dom_meta = json.loads(dom_meta_path.read_text(encoding="utf-8"))
                dom_meta.setdefault("files", {})[out_terms.name] = sha256(out_terms)
                dom_meta[f"hydrogen_v{batch}_term_count"] = len(term_rows)
                dom_meta_path.write_text(json.dumps(dom_meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(
        f"BATCH {batch} CORPUS: chat={len(buckets['chat']):,} written={len(buckets['written']):,} "
        f"mixed={len(buckets['mixed']):,} total={sum(len(v) for v in buckets.values()):,} stats={dict(stats)}"
    )
    if term_rows:
        print(f"BATCH {batch} TERMS: kept={len(term_rows):,} stats={dict(term_stats)} top_fields={field_counts.most_common(10)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("audit", "ingest"):
        p = sub.add_parser(name)
        p.add_argument("--batch", type=int, required=True)
    args = ap.parse_args()
    (cmd_audit if args.cmd == "audit" else cmd_ingest)(args)


if __name__ == "__main__":
    main()
