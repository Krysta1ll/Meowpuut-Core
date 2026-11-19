#!/usr/bin/env python3
"""Clean-licensed domain lexicon mining & orphan-anchor planner (MIT / project-owned only).

Subcommands:
  export-modern-seed  Writes curated modern AI/Med/Law/Fin/Slang/ACG/Tech seed terms (MIT).
  export-orphans      Identifies words missing from LM corpus & writes grouped anchor batches.
  merge-terms         Previews or merges DataSources/mimo-domains/*.tsv into Resources/Lexicon/terms.tsv.
  plan-corpus         Generates .build/colab-corpus/vertex_corpus_tasks.jsonl for Milestone 2.
"""
import argparse
import hashlib
import json
import lzma
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

DOMAINS_DIR = ROOT / "DataSources/mimo-domains"
DEFAULT_OUT_DIR = ROOT / ".build/colab-corpus"
DEFAULT_LEXICON_DIRS = (
    ROOT / ".build/lexicon-stage1",
    ROOT / ".build/lexicon",
    ROOT.parent / "Meowpuut-iOS/iOS/Generated",
    ROOT.parent.parent / "Meowpuut/iOS/Generated",
)
HAN_ONLY = re.compile(r"^[㐀-䶿一-鿿]{2,8}$")

# Curated 2023-2026 domain seed terms: (Traditional, Simplified, Field)
CURATED_SEEDS = [
    ("大模型", "大模型", "AI工程"), ("提示詞", "提示词", "AI工程"), ("微調", "微调", "AI工程"),
    ("蒸餾", "蒸馏", "AI工程"), ("向量資料庫", "向量数据库", "AI工程"), ("多模態", "多模态", "AI工程"),
    ("智能體", "智能体", "AI工程"), ("上下文視窗", "上下文窗口", "AI工程"), ("反向代理", "反向代理", "AI工程"),
    ("容器化", "容器化", "AI工程"), ("微服務", "微服务", "AI工程"), ("死鎖", "死锁", "AI工程"),
    ("記憶體洩漏", "记忆体泄漏", "AI工程"), ("內存洩漏", "内存泄漏", "AI工程"), ("低延遲", "低延迟", "AI工程"), ("零拷貝", "零拷贝", "AI工程"),
    ("神經網路", "神经网络", "AI工程"), ("注意力機制", "注意力机制", "AI工程"), ("推理引擎", "推理引擎", "AI工程"),
    ("算力", "算力", "AI工程"), ("過擬合", "过拟合", "AI工程"), ("檢索增強生成", "检索增强生成", "AI工程"),
    ("前綴快取", "前缀缓存", "AI工程"), ("斷點續訓", "断点续训", "AI工程"), ("灰度發布", "灰度发布", "AI工程"),
    ("詞向量", "词向量", "AI工程"), ("重排器", "重排器", "AI工程"), ("負載均衡", "负载均衡", "AI工程"),
    ("緩存穿透", "缓存穿透", "AI工程"), ("布隆過濾", "布隆过滤", "AI工程"), ("分詞器", "分词器", "AI工程"),
    ("越界訪問", "越界访问", "AI工程"), ("堆疊溢位", "堆栈溢出", "AI工程"), ("靜態檢查", "静态检查", "AI工程"),
    ("增量編譯", "增量编译", "AI工程"), ("映像檔", "映像档", "AI工程"), ("鏡像", "镜像", "AI工程"), ("降噪耳機", "降噪耳机", "AI工程"),
    ("高併發", "高并发", "AI工程"), ("邊界分支", "边界分支", "AI工程"), ("單元測試", "单元测试", "AI工程"),
    ("壓測", "压测", "AI工程"), ("端側模型", "端侧模型", "AI工程"), ("動態量化", "动态量化", "AI工程"),
    ("知識蒸餾", "知识蒸馏", "AI工程"), ("上下文推理", "上下文推理", "AI工程"), ("基準測試", "基准测试", "AI工程"),
    ("布洛芬", "布洛芬", "西醫"), ("阿莫西林", "阿莫西林", "西醫"), ("阿茲海默症", "阿兹海默症", "西醫"), ("阿爾茨海默症", "阿尔茨海默症", "西醫"),
    ("阿爾茨海默", "阿尔茨海默", "西醫"), ("核磁共振", "核磁共振", "西醫"), ("免疫抑制劑", "免疫抑制剂", "西醫"),
    ("胰島素", "胰岛素", "西醫"), ("心肌梗死", "心肌梗死", "西醫"), ("甲狀腺結節", "甲状腺结节", "西醫"),
    ("甘油三酯", "甘油三酯", "西醫"), ("乾眼症", "干眼症", "西醫"), ("骨密度", "骨密度", "西醫"),
    ("維生素", "维生素", "西醫"), ("緩釋膠囊", "缓释胶囊", "西醫"), ("過敏性鼻炎", "过敏性鼻炎", "西醫"),
    ("臨床試驗", "临床试验", "西醫"), ("代謝異常", "代谢异常", "西醫"), ("胃鏡檢查", "胃镜检查", "西醫"),
    ("彩超", "彩超", "西醫"), ("處方藥", "处方药", "西醫"), ("血糖儀", "血糖仪", "西醫"),
    ("血氧飽", "血氧饱", "西醫"), ("腦卒中", "脑卒中", "西醫"), ("尿酸高", "尿酸高", "西醫"),
    ("不當得利", "不当得利", "法律"), ("無罪推定", "无罪推定", "法律"), ("強制執行", "强制执行", "法律"),
    ("訴訟時效", "诉讼时效", "法律"), ("競業限制", "竞业限制", "法律"), ("勞動仲裁", "劳动仲裁", "法律"),
    ("侵權責任", "侵权责任", "法律"), ("抵押登記", "抵押登记", "法律"), ("提起公訴", "提起公诉", "法律"),
    ("時間戳", "时间戳", "法律"), ("違約條款", "违约条款", "法律"), ("授權書", "授权书", "法律"),
    ("量化寬鬆", "量化宽松", "金融"), ("資產負債表", "资产负债表", "金融"), ("市盈率", "市盈率", "金融"),
    ("流動性陷阱", "流动性陷阱", "金融"), ("融資融券", "融资融券", "金融"), ("風險敞口", "风险敞口", "金融"),
    ("現金流量表", "现金流量表", "金融"), ("指數基金", "指数基金", "金融"), ("美聯儲", "美联储", "金融"),
    ("不良貸款率", "不良贷款率", "金融"), ("可轉債", "可转债", "金融"), ("殖利率倒掛", "殖利率倒挂", "金融"),
    ("天使融資", "天使融资", "金融"), ("跨境電商", "跨境电商", "金融"), ("毛利率", "毛利率", "金融"),
    ("顯眼包", "显眼包", "網絡熱梗"), ("電子榨菜", "电子榨菜", "網絡熱梗"), ("情緒價值", "情绪价值", "網絡熱梗"),
    ("脆皮大學生", "脆皮大学生", "網絡熱梗"), ("鬆弛感", "松弛感", "網絡熱梗"), ("班味", "班味", "網絡熱梗"),
    ("已讀亂回", "已读乱回", "網絡熱梗"), ("硬控", "硬控", "網絡熱梗"), ("草台班子", "草台班子", "網絡熱梗"),
    ("紅溫", "红温", "網絡熱梗"), ("破防", "破防", "網絡熱梗"), ("精神內耗", "精神内耗", "網絡熱梗"),
    ("水靈靈", "水灵灵", "網絡熱梗"), ("偷感", "偷感", "網絡熱梗"), ("人機感", "人机感", "網絡熱梗"),
    ("活人感", "活人感", "網絡熱梗"), ("祛魅", "祛魅", "網絡熱梗"), ("早八", "早八", "網絡熱梗"),
    ("續命", "续命", "網絡熱梗"), ("白月光", "白月光", "網絡熱梗"), ("複製人", "复制人", "網絡熱梗"),
    ("語音條", "语音条", "網絡熱梗"), ("梗圖", "梗图", "網絡熱梗"), ("吃土", "吃土", "網絡熱梗"),
    ("種草", "种草", "網絡熱梗"), ("避雷", "避雷", "網絡熱梗"), ("爆改", "爆改", "網絡熱梗"),
    ("封神", "封神", "網絡熱梗"), ("摸魚", "摸鱼", "網絡熱梗"), ("躺平", "躺平", "網絡熱梗"),
    ("收藏夾", "收藏夹", "網絡熱梗"), ("鍵帽", "键帽", "網絡熱梗"), ("機械鍵盤", "机械键盘", "網絡熱梗"),
    ("短視頻", "短视频", "網絡熱梗"), ("網紅店", "网红店", "網絡熱梗"), ("搭子", "搭子", "網絡熱梗"),
    ("雞排", "鸡排", "網絡熱梗"), ("珍奶", "珍奶", "網絡熱梗"), ("社恐", "社恐", "網絡熱梗"), ("社牛", "社牛", "網絡熱梗"),
    ("崩壞星穹鐵道", "崩坏星穹铁道", "ACG"), ("絕區零", "绝区零", "ACG"), ("鳴潮", "鸣潮", "ACG"),
    ("黑神話悟空", "黑神话悟空", "ACG"), ("芙莉蓮", "芙莉莲", "ACG"), ("孤獨搖滾", "孤独摇滚", "ACG"),
    ("咒術迴戰", "咒术回战", "ACG"), ("初音未來", "初音未来", "ACG"), ("番劇", "番剧", "ACG"),
    ("追番", "追番", "ACG"), ("催更", "催更", "ACG"), ("肝帝", "肝帝", "ACG"),
    ("歐皇", "欧皇", "ACG"), ("非酋", "非酋", "ACG"), ("大保底", "大保底", "ACG"),
    ("聖遺物", "圣遗物", "ACG"), ("配隊", "配队", "ACG"), ("吃谷", "吃谷", "ACG"),
    ("痛包", "痛包", "ACG"), ("吧唧", "吧唧", "ACG"), ("單推", "单推", "ACG"),
    ("聲優", "声优", "ACG"), ("立繪", "立绘", "ACG"), ("同人誌", "同人志", "ACG"),
    ("發糖", "发糖", "ACG"), ("滿命", "满命", "ACG"), ("深淵滿星", "深渊满星", "ACG"),
    ("前瞻直播", "前瞻直播", "ACG"), ("兌換碼", "兑换码", "ACG"), ("特典小卡", "特典小卡", "ACG"),
    ("清體力", "清体力", "ACG"), ("十連雙金", "十连双金", "ACG"), ("雙暴詞條", "双暴词条", "ACG"),
    ("虛擬主播", "虚拟主播", "ACG"), ("復刻", "复刻", "ACG"), ("立牌", "立牌", "ACG"),
    ("手辦", "手办", "ACG"), ("周邊", "周边", "ACG"), ("走位技巧", "走位技巧", "ACG"),
    ("二周目", "二周目", "ACG"), ("公會戰", "公会战", "ACG"), ("搖桿飄移", "摇杆飘移", "ACG"),
    ("應援", "应援", "ACG"), ("音遊", "音游", "ACG"), ("建模", "建模", "ACG"),
    ("人形機器人", "人形机器人", "時事科技"), ("低空經濟", "低空经济", "時事科技"), ("具身智能", "具身智能", "時事科技"),
    ("純血鴻蒙", "纯血鸿蒙", "時事科技"), ("三摺疊屏", "三折叠屏", "時事科技"), ("摺疊屏", "折叠屏", "時事科技"),
    ("新能源車", "新能源车", "時事科技"), ("智駕", "智驾", "時事科技"), ("國補", "国补", "時事科技"),
    ("特種兵旅遊", "特种兵旅游", "時事科技"), ("穀子經濟", "谷子经济", "時事科技"), ("微短劇", "微短剧", "時事科技"),
    ("主理人", "主理人", "時事科技"), ("腦機介面", "脑机接口", "時事科技"), ("腦機接口", "脑机接口", "時事科技"),
    ("靈動島", "灵动岛", "時事科技"), ("空間計算", "空间计算", "時事科技"), ("以舊換新", "以旧换新", "時事科技"),
    ("首發經濟", "首发经济", "時事科技"), ("銀髮經濟", "银发经济", "時事科技"), ("碳中和", "碳中和", "時事科技"),
    ("無人機配送", "无人机配送", "時事科技"), ("頭顯", "头显", "時事科技"), ("筆記本", "笔记本", "時事科技"),
]


def is_clean_word(hans: str) -> bool:
    return bool(HAN_ONLY.fullmatch(hans)) and not blocked_terms.zh_hit(hans) and not blocked_terms.drop_hit(hans)


def convert_lines(words: list[str], converter: str | None, traditional: bool = False) -> dict[str, str]:
    if not words or not converter:
        return {w: w for w in words}
    cmd = [converter, "--traditional"] if traditional else [converter]
    out = subprocess.run(cmd, input="\n".join(words) + "\n", text=True, capture_output=True, check=True).stdout.splitlines()
    if len(out) != len(words):
        raise SystemExit(f"converter length mismatch: {len(out)} != {len(words)}")
    return dict(zip(words, out))


def write_seed_manifest(extra: dict):
    manifest_path = DOMAINS_DIR / "SOURCE.json"
    sources = {"modern-seed-terms.tsv": "Project-authored 2023-2026 seed terms (MIT)"}
    if (DOMAINS_DIR / "modern-llm-terms.tsv").exists():
        sources["modern-llm-terms.tsv"] = "Project-owned LLM-written domain terms (MIT)"
    files = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(DOMAINS_DIR.glob("*.tsv"))}
    data = {
        "name": "mimo-domains",
        "license": "written for this project; MIT (no third-party text)",
        "sources": sources,
        "files": files,
        **extra,
    }
    manifest_path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def cmd_export_modern_seed(_args):
    DOMAINS_DIR.mkdir(parents=True, exist_ok=True)
    out = DOMAINS_DIR / "modern-seed-terms.tsv"
    seen = set()
    rows = ["# Curated 2023-2026 AI/Med/Law/Fin/Slang/ACG/Tech terms (Traditional, Simplified, Field)"]
    kept = 0
    for trad, simp, field in CURATED_SEEDS:
        if not is_clean_word(simp) or not is_clean_word(trad) or (trad, simp) in seen:
            continue
        seen.add((trad, simp))
        rows.append(f"{trad}\t{simp}\t{field}")
        kept += 1
    out.write_text("\n".join(rows) + "\n", encoding="utf-8")
    write_seed_manifest({"modern_seed_count": kept})
    print(f"export-modern-seed: wrote {kept} clean terms -> {out}")


def resolve_lexicon_dir(explicit: str | None) -> Path:
    candidates = [Path(explicit)] if explicit else list(DEFAULT_LEXICON_DIRS)
    for c in candidates:
        if (c / "native-pinyin.tsv").exists():
            return c
    raise SystemExit("native-pinyin.tsv not found; pass --lexicon-dir <path>")


def load_pair_counts(path: Path) -> dict[str, int]:
    counts = {}
    if not path.exists():
        return counts
    with lzma.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            if line.startswith("U\t"):
                p = line.rstrip("\n").split("\t")
                if len(p) >= 3 and p[2].isdigit():
                    counts[p[1]] = int(p[2])
    return counts


def classify_bucket(field: str) -> tuple[str, str]:
    if any(k in field for k in ("熱梗", "热梗", "ACG", "食物", "飲食", "饮食", "動物", "动物", "群聊", "口語", "口语", "寵物", "宠物")):
        return ("spoken", "zh-TW" if any(k in field for k in ("繁", "台", "珍奶", "雞排")) else "zh-CN")
    return ("written", "zh-TW" if "繁" in field else "zh-CN")


def cmd_export_orphans(args):
    lex_dir = resolve_lexicon_dir(args.lexicon_dir)
    lm_counts = load_pair_counts(ROOT / "DataSources/mimo-pairs/counts.txt.xz")
    lex_words = {}
    with open(lex_dir / "native-pinyin.tsv", "r", encoding="utf-8") as f:
        next(f)
        for line in f:
            if line == "# context\n":
                break
            p = line.rstrip("\n").split("\t")
            if len(p) >= 5 and p[4].lstrip("-").isdigit() and is_clean_word(p[3]):
                lex_words[p[3]] = max(lex_words.get(p[3], -9999), int(p[4]))

    field_buckets: dict[str, list[tuple[str, str]]] = defaultdict(list)
    seen_words = set()
    for fp in sorted(DOMAINS_DIR.glob("*.tsv")) + [ROOT / "Resources/Lexicon/terms.tsv"]:
        if not fp.exists():
            continue
        for line in fp.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 3:
                continue
            trad, simp, field = parts[0].strip(), parts[1].strip(), parts[2].strip()
            if simp in seen_words or not is_clean_word(simp) or not is_clean_word(trad):
                continue
            if lm_counts.get(simp, 0) < args.max_lm_count:
                seen_words.add(simp)
                field_buckets[field].append((trad, simp))

    if args.include_lexicon_orphans:
        for hans in sorted(lex_words, key=lambda w: (-lex_words[w], len(w), w)):
            if hans in seen_words or len(hans) < 2 or lm_counts.get(hans, 0) >= args.max_lm_count:
                continue
            seen_words.add(hans)
            field_buckets["通用高頻補詞"].append((hans, hans))
            if len(seen_words) >= args.max_unique_anchors:
                break

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    out_jsonl = out_dir / "anchor_batches.jsonl"
    batches, idx = [], 0
    for field, pairs in sorted(field_buckets.items()):
        for start in range(0, len(pairs), args.group_size):
            chunk = pairs[start:start + args.group_size]
            if len(chunk) < 3:
                continue
            for script in ("zh-CN", "zh-TW"):
                reg, _ = classify_bucket(field)
                idx += 1
                batches.append({
                    "id": f"b{idx:05d}",
                    "field": field,
                    "script": script,
                    "register": reg,
                    "anchors": [s if script == "zh-CN" else t for t, s in chunk],
                })
    with open(out_jsonl, "w", encoding="utf-8") as f:
        for b in batches:
            f.write(json.dumps(b, ensure_ascii=False) + "\n")
    zero_ev = sum(1 for w in lex_words if lm_counts.get(w, 0) == 0)
    print(f"export-orphans: lexicon={len(lex_words):,} (zero_lm={zero_ev:,}), anchor_words={len(seen_words):,}, batches={len(batches):,} -> {out_jsonl}")


def cmd_merge_terms(args):
    terms_path = ROOT / "Resources/Lexicon/terms.tsv"
    existing = terms_path.read_text(encoding="utf-8").splitlines()
    seen = set()
    for line in existing:
        if line.strip() and not line.startswith("#"):
            p = line.split("\t")
            if len(p) >= 2:
                seen.add((p[0].strip(), p[1].strip()))
    candidates = []
    for fp in sorted(DOMAINS_DIR.glob("*.tsv")):
        for line in fp.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            p = [c.strip() for c in line.split("\t")]
            if len(p) >= 3 and len(p[0]) == len(p[1]) and is_clean_word(p[1]):
                candidates.append((p[0], p[1], p[2]))
    if args.converter and candidates:
        t_map = convert_lines([s for _, s, _ in candidates], args.converter, traditional=True)
        s_map = convert_lines([t for t, _, _ in candidates], args.converter, traditional=False)
        candidates = [(t if t != s else t_map[s], s if t != s else s_map[t], f) for t, s, f in candidates]
    added = []
    for t, s, f in sorted(candidates):
        if len(t) == len(s) and is_clean_word(t) and is_clean_word(s) and (t, s) not in seen:
            seen.add((t, s))
            added.append(f"{t}\t{s}\t{f}")
    if args.apply and added:
        raw = terms_path.read_text(encoding="utf-8")
        prefix = "" if raw.endswith("\n") else "\n"
        with open(terms_path, "a", encoding="utf-8") as f:
            f.write(prefix + "\n".join(added) + "\n")
    print(f"merge-terms [{'APPLIED' if args.apply else 'DRY-RUN'}]: {len(added)} new entries -> {terms_path}")


def cmd_plan_corpus(args):
    """Combines anchor_batches (new domain + orphan words) with rich modern-topic & disambiguation tasks."""
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    anchors_path = Path(args.anchors)
    tasks, tid = [], 0
    topic_specs = [
        ("2023-2026網絡熱梗群聊", "spoken", 60), ("ACG手遊抽卡與動畫討論", "spoken", 60),
        ("職場辦公與程式開發吐槽", "spoken", 55), ("日常餐飲外送與消費點單", "spoken", 55),
        ("AI大模型與軟硬體技術討論", "written", 50), ("新能源車智駕與科技趨勢", "written", 45),
        ("臨床西醫用藥與健康體檢", "written", 45), ("金融理財財報與勞動法律", "written", 45),
        ("寵物養貓與生活日常閒聊", "spoken", 45), ("旅遊交通訂票與住宿體驗", "spoken", 40),
        ("語境消歧同音詞自然對比造句", "spoken", 55), ("動賓搭配與量詞精準運用例句", "written", 55),
    ]
    angles = ["清晨通勤", "深夜加班", "週末聚會", "開箱評測", "朋友私聊", "群組討論", "門診諮詢", "專案復盤", "購物比價", "遊戲開荒"]
    for topic, reg, count in topic_specs:
        for i in range(count):
            for script in ("zh-TW", "zh-CN"):
                tid += 1
                tasks.append({
                    "id": f"tp{tid:05d}", "kind": "topic", "field": topic,
                    "angle": angles[(i + tid) % len(angles)], "script": script, "register": reg,
                })
    if anchors_path.exists():
        raw_anchors = [json.loads(line) for line in anchors_path.read_text(encoding="utf-8").splitlines() if line.strip()]
        for b in raw_anchors[:args.max_anchor_tasks]:
            tid += 1
            tasks.append({"id": f"an{tid:05d}", "kind": "anchor", **b})
    out_path.write_text("\n".join(json.dumps(t, ensure_ascii=False) for t in tasks) + "\n", encoding="utf-8")
    print(f"plan-corpus: {len(tasks)} tasks written -> {out_path}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("export-modern-seed")
    p_or = sub.add_parser("export-orphans")
    p_or.add_argument("--lexicon-dir", default=None)
    p_or.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR))
    p_or.add_argument("--group-size", type=int, default=8)
    p_or.add_argument("--max-lm-count", type=int, default=3)
    p_or.add_argument("--max-unique-anchors", type=int, default=28000)
    p_or.add_argument("--include-lexicon-orphans", action="store_true")
    p_mg = sub.add_parser("merge-terms")
    p_mg.add_argument("--apply", action="store_true")
    p_mg.add_argument("--converter", default=None)
    p_pc = sub.add_parser("plan-corpus")
    p_pc.add_argument("--anchors", default=str(DEFAULT_OUT_DIR / "anchor_batches.jsonl"))
    p_pc.add_argument("--out", default=str(DEFAULT_OUT_DIR / "vertex_corpus_tasks.jsonl"))
    p_pc.add_argument("--max-anchor-tasks", type=int, default=4200)
    args = ap.parse_args()
    {"export-modern-seed": cmd_export_modern_seed, "export-orphans": cmd_export_orphans,
     "merge-terms": cmd_merge_terms, "plan-corpus": cmd_plan_corpus}[args.cmd](args)


if __name__ == "__main__":
    main()
