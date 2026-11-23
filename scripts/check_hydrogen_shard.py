#!/usr/bin/env python3
"""Strict anti-template, zero-leak, zero-blocked validator for hydrogen-low corpus shards.

Usage:
  python3 scripts/check_hydrogen_shard.py <shard_dir> [--clean-in-place]
"""
import argparse
import lzma
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

HAN = re.compile(r"[㐀-䶿一-鿿]")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")
MIN_LEN, MAX_LEN = 6, 40


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


def load_existing_lines() -> set[str]:
    seen: set[str] = set()
    ev_dir = ROOT / "DataSources/mimo-everyday"
    for xp in sorted(ev_dir.glob("*.txt.xz")) if ev_dir.exists() else []:
        with lzma.open(xp, "rt", encoding="utf-8") as f:
            seen.update(f.read().splitlines())
    return seen


def file_diversity(lines: list[str]) -> tuple[float, float]:
    if not lines:
        return 0.0, 1.0
    s4, g3 = Counter(), Counter()
    total_g3 = 0
    for ln in lines:
        han = "".join(HAN.findall(ln))
        if len(han) >= 4:
            s4[han[-4:]] += 1
        for i in range(max(0, len(han) - 2)):
            g3[han[i : i + 3]] += 1
            total_g3 += 1
    top4_suf = (s4.most_common(1)[0][1] / len(lines)) if s4 else 0.0
    g3div = (len(g3) / total_g3) if total_g3 else 0.0
    return g3div, top4_suf


def validate_shard(shard_dir: Path, clean_in_place: bool) -> dict:
    t2s = load_t2s()
    eval_grams = load_eval_grams(t2s)
    seen = load_existing_lines()
    blocked_re = blocked_terms.counted()

    total_raw = 0
    kept_total = 0
    dropped = Counter()
    prefix2 = Counter()
    per_file = {}
    rejected_templates = []

    files = sorted(shard_dir.glob("*.txt"))
    for fp in files:
        kept_lines = []
        for raw in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            if not raw.strip():
                continue
            total_raw += 1
            s = LIST_PREFIX.sub("", raw).strip().strip("「」\"'")
            if not (MIN_LEN <= len(s) <= MAX_LEN):
                dropped["bad_length"] += 1
                continue
            if BROKEN.search(s) or len(HAN.findall(s)) < 4:
                dropped["broken_or_too_few_han"] += 1
                continue
            if s in seen:
                dropped["duplicate"] += 1
                continue
            han_only = "".join(HAN.findall(s))
            if blocked_re.search(s):
                dropped["blocked_term"] += 1
                continue
            if any(
                x[i : i + 5] in eval_grams
                for x in (han_only, han_only.translate(t2s))
                for i in range(max(0, len(x) - 4))
            ):
                dropped["eval_overlap"] += 1
                continue
            seen.add(s)
            kept_lines.append(s)

        g3div, suf4 = file_diversity(kept_lines)
        if len(kept_lines) >= 40 and (g3div < 0.52 or suf4 > 0.07):
            rejected_templates.append((fp.name, len(kept_lines), round(g3div, 2), round(suf4, 3)))
            dropped["template_file_rejected"] += len(kept_lines)
            per_file[fp.name] = f"REJECTED_TEMPLATE(g3div={g3div:.2f},suf4={suf4:.1%})"
            if clean_in_place:
                fp.unlink(missing_ok=True)
            continue

        for s in kept_lines:
            han_only = "".join(HAN.findall(s))
            if len(han_only) >= 2:
                prefix2[han_only[:2]] += 1
        per_file[fp.name] = f"{len(kept_lines)} (g3div={g3div:.2f})"
        kept_total += len(kept_lines)
        if clean_in_place:
            fp.write_text("\n".join(kept_lines) + ("\n" if kept_lines else ""), encoding="utf-8")

    top_prefix = prefix2.most_common(5)
    top_share = (top_prefix[0][1] / kept_total) if kept_total and top_prefix else 0.0
    return {
        "files": len(files),
        "raw": total_raw,
        "kept": kept_total,
        "dropped": dict(dropped),
        "rejected_templates": rejected_templates,
        "top_2char_start_share": round(top_share, 4),
        "top_starts": top_prefix,
        "per_file": per_file,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shard_dir")
    ap.add_argument("--clean-in-place", action="store_true")
    args = ap.parse_args()
    res = validate_shard(Path(args.shard_dir), args.clean_in_place)
    print(f"files={res['files']} raw={res['raw']} kept={res['kept']} dropped={res['dropped']}")
    if res["rejected_templates"]:
        print(f"REJECTED_TEMPLATES={res['rejected_templates']}")
    print(f"top_2char_start_share={res['top_2char_start_share']:.2%} top_starts={res['top_starts']}")
    for name, info in res["per_file"].items():
        print(f"  {name}: {info}")


if __name__ == "__main__":
    main()
