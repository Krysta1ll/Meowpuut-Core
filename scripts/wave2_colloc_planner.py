#!/usr/bin/env python3
"""Wave-2 extreme collocation & spoken-register corpus planner (0 eval leakage).

Prioritizes:
  Wave-2A: 3,600 spoken chat/IM tasks across 10 modern/domain categories (feeds 30x spoken boost).
  Wave-2B: 2,400 high-confusion homophone & bridged-collocation tasks (feeds Stage-2 Trigrams).
  Wave-2C: 4,800 anchor-forced tasks targeting resurrected terms.tsv words, low-spoken terms & orphans.
"""
import argparse
import json
import lzma
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import colab_lexicon_mine as mine  # noqa: E402

DOMAINS_2A = [
    "AI大模型提示詞微調與向量檢索實戰對話",
    "軟體後端併發容器部署日誌與除錯群聊",
    "臨床西醫看診檢查單數值與處方用藥交流",
    "民商合約勞動仲裁智慧財產權職場討論",
    "財報毛利率市盈率基金可轉債投資群聊",
    "2023至2026最新網路熱梗脆社群吐槽日常",
    "ACG二次元手遊抽卡配隊追番漫展周邊聊",
    "新能源車智駕人形機器人摺疊螢幕科技聊",
    "居家生活網購退換外送點餐咖啡養貓日常",
    "台灣與大陸高頻日常習慣用語切換自然對話",
]
ANGLES_2A = [
    "早八通勤傳訊", "深夜加班吐槽", "朋友群組討論", "私聊問問題", "開箱實測分享",
    "醫院回診討論", "專案覆盤會議", "週末逛街聚會", "遊戲組隊語音", "租屋買房議價",
    "看診領藥提醒", "財經行情快評", "線下漫展repo", "養貓日常炫娃", "點外送踩雷評價",
    "系統線上報警", "程式碼審查建議", "合約條款確認",
]
COLLOC_2B = [
    ("AI系統除錯動賓搭配", "的/了/把/給/再/又"),
    ("醫療健康症狀與用藥量詞", "個/盒/粒/顆/次/先"),
    ("法律合約財務審計固定搭配", "向/對/為/將/已/未"),
    ("網路社群熱梗與情緒副詞", "很/太/超/都/就/也"),
    ("ACG手遊動畫高頻動詞搭配", "抽/歪/肝/刷/打/補"),
    ("日常工作簡報排程高頻字區分", "在/再/作/做/得/地"),
    ("交通外送購物時間地點搭配", "到/去/從/往/剛/快"),
    ("科技數碼硬體規格與量詞搭配", "台/顆/條/張/核/倍"),
]


def load_counts_detail(path: Path) -> dict[str, tuple[int, int]]:
    out = {}
    if not path.exists():
        return out
    with lzma.open(path, "rt", encoding="utf-8") as f:
        for ln in f:
            if ln.startswith("U\t"):
                p = ln.rstrip("\n").split("\t")
                if len(p) >= 5 and p[2].isdigit() and p[4].isdigit():
                    out[p[1]] = (int(p[2]), int(p[4]))
    return out


def build_wave2_tasks(
    lex_dir: Path,
    terms_path: Path,
    counts_path: Path,
    n_2a: int = 3600,
    n_2b: int = 2400,
    n_2c: int = 4800,
) -> list[dict]:
    lm = load_counts_detail(counts_path)
    lex_weights: dict[str, int] = {}
    with open(lex_dir / "native-pinyin.tsv", "r", encoding="utf-8") as f:
        next(f)
        for ln in f:
            if ln == "# context\n":
                break
            p = ln.rstrip("\n").split("\t")
            if len(p) >= 5 and p[4].lstrip("-").isdigit() and mine.is_clean_word(p[3]):
                lex_weights[p[3]] = max(lex_weights.get(p[3], 0), int(p[4]))

    tasks, tid = [], 0

    # Wave-2A: rich spoken multi-domain chat (alternating zh-TW & zh-CN)
    pairs_2a = n_2a // 2
    for i in range(pairs_2a):
        dom = DOMAINS_2A[i % len(DOMAINS_2A)]
        ang = ANGLES_2A[(i + i // len(DOMAINS_2A)) % len(ANGLES_2A)]
        for sc in ("zh-TW", "zh-CN"):
            tid += 1
            tasks.append({"id": f"w2a{tid:05d}", "kind": "topic", "field": dom, "angle": ang, "script": sc, "register": "spoken"})

    # Wave-2B: collocation & bridged homophone drills (65% spoken, 35% written)
    pairs_2b = n_2b // 2
    for i in range(pairs_2b):
        fld, bridges = COLLOC_2B[i % len(COLLOC_2B)]
        reg = "spoken" if (i % 20) < 13 else "written"
        ang = f"{ANGLES_2A[i % len(ANGLES_2A)]}｜高頻橋接字：{bridges}"
        for sc in ("zh-TW", "zh-CN"):
            tid += 1
            tasks.append({"id": f"w2b{tid:05d}", "kind": "colloc", "field": fld, "angle": ang, "script": sc, "register": reg})

    # Wave-2C: Tiered Anchor-Forcing prioritizing newly resurrected & low-spoken terms.tsv words
    term_rows, seen_simp = [], set()
    for ln in terms_path.read_text(encoding="utf-8").splitlines():
        if not ln.strip() or ln.startswith("#"):
            continue
        p = ln.split("\t")
        if len(p) < 3:
            continue
        t, s, f = p[0].strip(), p[1].strip(), p[2].strip()
        if len(t) == len(s) and mine.is_clean_word(t) and mine.is_clean_word(s) and s not in seen_simp and s in lex_weights:
            seen_simp.add(s)
            tot, spk = lm.get(s, (0, 0))
            term_rows.append((spk, tot, len(s), f, t, s))

    # Priority: spk == 0 first, then low tot, shorter compound words first
    term_rows.sort(key=lambda r: (0 if r[0] == 0 else (1 if r[0] < 3 else 2), r[1], r[2], r[5]))

    anchor_groups = []
    # Group by domain field for top priority terms
    by_tier_field: dict[str, list[tuple[str, str]]] = {}
    for spk, tot, _slen, f, t, s in term_rows:
        if spk < 5 or tot < 20:
            by_tier_field.setdefault(f, []).append((t, s))

    for f in sorted(by_tier_field):
        items = by_tier_field[f]
        for st in range(0, len(items), 7):
            chunk = items[st : st + 7]
            if len(chunk) >= 3:
                anchor_groups.append((f, chunk))

    # Append general unseen lexicon orphans (2..4 Han chars, high prior weight)
    orphans = [
        (w, w) for w in sorted(lex_weights, key=lambda x: (-lex_weights[x], len(x), x))
        if w not in seen_simp and 2 <= len(w) <= 4 and lm.get(w, (0, 0))[1] == 0
    ]
    for st in range(0, len(orphans), 7):
        chunk = orphans[st : st + 7]
        if len(chunk) >= 3:
            anchor_groups.append(("通用高頻搭配詞", chunk))

    idx = 0
    for f, chunk in anchor_groups:
        for sc in ("zh-TW", "zh-CN"):
            if idx >= n_2c:
                break
            reg = "spoken" if (idx % 4) < 3 else "written"
            tid += 1
            idx += 1
            tasks.append({
                "id": f"w2c{tid:05d}",
                "kind": "anchor",
                "field": f,
                "script": sc,
                "register": reg,
                "anchors": [t if sc == "zh-TW" else s for t, s in chunk],
            })
        if idx >= n_2c:
            break
    return tasks


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--lexicon-dir", default=str(ROOT / ".build/lexicon-stage2"))
    ap.add_argument("--terms", default=str(ROOT / "Resources/Lexicon/terms.tsv"))
    ap.add_argument("--counts", default=str(ROOT / "DataSources/mimo-pairs/counts.txt.xz"))
    ap.add_argument("--out", default=str(ROOT / ".build/colab-corpus/wave2_corpus_tasks.jsonl"))
    ap.add_argument("--n-2a", type=int, default=3600)
    ap.add_argument("--n-2b", type=int, default=2400)
    ap.add_argument("--n-2c", type=int, default=4800)
    args = ap.parse_args()
    tasks = build_wave2_tasks(
        Path(args.lexicon_dir), Path(args.terms), Path(args.counts), args.n_2a, args.n_2b, args.n_2c
    )
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(json.dumps(t, ensure_ascii=False) for t in tasks) + "\n", encoding="utf-8")
    counts = {k: sum(1 for t in tasks if t["id"].startswith(k)) for k in ("w2a", "w2b", "w2c")}
    spk_n = sum(1 for t in tasks if t["register"] == "spoken")
    print(f"wave2-plan: total={len(tasks):,} {counts} spoken={spk_n:,} written={len(tasks)-spk_n:,} -> {out}")


if __name__ == "__main__":
    main()
