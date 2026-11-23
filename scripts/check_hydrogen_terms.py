#!/usr/bin/env python3
"""Validator for hydrogen-low multi-batch domain/ACG/meme lexicon term TSV shards.

Usage:
  python3 scripts/check_hydrogen_terms.py <shard_dir> [--clean-in-place]
"""
import argparse
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

HAN_WORD = re.compile(r"^[㐀-䶿一-鿿]{2,8}$")
CLEAN_CELL = re.compile(r"[\s0-9.\-•*]+")
BAD_FILLER_TAIL = set("呀啊呢吧啦嗎吗嘛哦喲哟")


def load_existing_terms() -> set[tuple[str, str]]:
    seen: set[tuple[str, str]] = set()
    dom_dir = ROOT / "DataSources/mimo-domains"
    for fp in sorted(dom_dir.glob("*.tsv")) if dom_dir.exists() else []:
        for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                seen.add((cols[0].strip(), cols[1].strip()))
    terms_tsv = ROOT / "Resources/Lexicon/terms.tsv"
    if terms_tsv.exists():
        for ln in terms_tsv.read_text(encoding="utf-8", errors="ignore").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                seen.add((cols[0].strip(), cols[1].strip()))
    return seen


def validate(shard_dir: Path, clean_in_place: bool) -> dict:
    seen = load_existing_terms()
    dropped = Counter()
    kept_total = 0
    raw_total = 0
    per_file = {}
    for fp in sorted(shard_dir.glob("*.tsv")):
        kept = []
        for ln in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            s_ln = ln.strip()
            if not s_ln or s_ln.startswith("#"):
                continue
            raw_total += 1
            parts = [p.strip() for p in s_ln.split("\t")]
            if len(parts) < 2:
                dropped["bad_cols"] += 1
                continue
            t = CLEAN_CELL.sub("", parts[0])
            s = CLEAN_CELL.sub("", parts[1])
            field = parts[2].strip() if len(parts) >= 3 and parts[2].strip() else fp.stem
            if len(t) != len(s) or not HAN_WORD.fullmatch(t) or not HAN_WORD.fullmatch(s):
                dropped["bad_han_or_len"] += 1
                continue
            if len(t) >= 4 and t[-1] in BAD_FILLER_TAIL:
                dropped["filler_tail"] += 1
                continue
            if blocked_terms.zh_hit(s) or blocked_terms.drop_hit(s) or blocked_terms.zh_hit(t) or blocked_terms.drop_hit(t):
                dropped["blocked"] += 1
                continue
            if (t, s) in seen:
                dropped["duplicate"] += 1
                continue
            seen.add((t, s))
            kept.append(f"{t}\t{s}\t{field}")
        per_file[fp.name] = len(kept)
        kept_total += len(kept)
        if clean_in_place:
            fp.write_text("\n".join(kept) + ("\n" if kept else ""), encoding="utf-8")
    return {"files": len(per_file), "raw": raw_total, "kept": kept_total, "dropped": dict(dropped), "per_file": per_file}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shard_dir")
    ap.add_argument("--clean-in-place", action="store_true")
    args = ap.parse_args()
    res = validate(Path(args.shard_dir), args.clean_in_place)
    print(f"files={res['files']} raw={res['raw']} kept={res['kept']} dropped={res['dropped']}")
    for name, cnt in res["per_file"].items():
        print(f"  {name}: {cnt}")


if __name__ == "__main__":
    main()
