#!/usr/bin/env python3
"""Cost-optimized Vertex AI Gemini 4 Hydrogen Batch corpus pipeline (HARD CAP <= $100 USD).

Cost-reduction design:
  1. Global endpoint Batch Prediction: 50% Batch discount + 10% global discount ($0.75 in / $3.75 out per 1M).
  2. Zero thinking-token waste: thinkingLevel=LOW + explicit no-analysis directive (empirically 0 thought tokens).
  3. Dense packing (260 lines/req, 10-24 chars, zero numbering/headers/blank lines): <0.5% input overhead.
  4. Shared compact systemInstruction per script for implicit prefix caching.
  5. Hard budget guard calibrated from real pilot token counts so spend stays strictly <= $100 USD.
"""
import argparse
import hashlib
import json
import lzma
import os
import random
import re
import ssl
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

PROJECT = "gen-lang-client-0285872710"
LOCATION = "global"
MODEL = "publishers/google/models/gemini-4-hydrogen"
BUCKET = "meowpuut-hk-kb-private-0285872710"

BATCH_IN_PER_M = 0.75
BATCH_OUT_PER_M = 3.75
HARD_CAP_USD = 100.0

SYS_TW = (
    "你是中文輸入法語料編寫員。全句自然台灣繁體中文與台灣日常用語，"
    "絕不加序號、標題、引號、句末句號或空行，每行一句10-24字，嚴禁模板套句、真人名人與敏感詞。"
    "直接開始輸出，勿做任何分析。"
)
SYS_CN = (
    "你是中文输入法语料编写员。全句自然中国大陆简体中文与常见表达，"
    "绝不加序号、标题、引号、句末句号或空行，每行一句10-24字，严禁模板套句、真人名人与敏感词。"
    "直接开始输出，勿做任何分析。"
)

# (bucket_tag, script, register, weight, topics)
CATALOG = [
    ("tc_chat", "zh-TW", "spoken", 0.22, [
        "手搖飲點餐甜度冰塊與辦公室團購下午茶", "超商微波美食咖啡寄杯與深夜宵夜選擇",
        "捷運公車通勤塞車高鐵搶票與轉乘日常", "租屋看房房東修繕水電帳單與室友互動",
        "週末約吃火鍋燒肉早午餐排隊候位聊天", "網購蝦皮包裹到貨取件退換貨與比價",
        "上班開會摸魚請假加班補休與同事八卦", "家裡養貓養狗鏟屎看獸醫與寵物搗蛋",
        "下雨天忘記帶傘寒流穿搭防曬與空氣品質", "朋友出國自由行日本韓國代購行程規劃",
        "手機沒電行動電源藍牙耳機與電信資費", "健身房重訓慢跑減脂飲食控制與痠痛",
        "看電影追劇搶演唱會門票與假日宅家", "感冒過敏看診領藥晚睡失眠與咖啡續命",
        "家庭聚餐長輩群組過節送禮與生活瑣事", "感情曖昧暈船約會踩雷與朋友情感諮詢",
        "大學選課報告分組考試熬夜與宿舍日常", "美妝護膚防曬剪頭髮染髮與穿搭討論",
        "開車停車找車位加油保養與違規罰單吐槽", "存錢記帳信用卡回饋分期與月底吃土",
    ]),
    ("sc_chat", "zh-CN", "spoken", 0.22, [
        "点外卖拼好饭奶茶咖啡凑单满减聊天", "早高峰地铁挤车打车拼车高铁抢票日常",
        "租房合租找中介修家电交水电燃气费", "周末约火锅烧烤夜市探店排队等位吐槽",
        "网购淘宝京东拼多多凑单退货取快递", "打工人早八打卡摸鱼请假加班年终奖吐槽",
        "家里养猫养狗遛弯买猫粮看宠物医院", "天气降温暴雨忘带伞换季穿搭与取暖",
        "假期旅游特种兵打卡民宿避坑与攻略", "手机续航充电宝流量套餐与换机讨论",
        "健身减脂夜跑帕梅拉轻食外卖与自律", "追剧看综艺抢演唱会门票与周末宅家",
        "熬夜失眠感冒发烧买药体检与养生茶", "家庭群聊回家过年催婚相亲与走亲戚",
        "恋爱约会冷战闺蜜吐槽与社交边界感", "大学早八赶论文小组作业期末复习日常",
        "理发烫染护肤防晒化妆踩雷与种草分享", "开车堵车找车位新能源充电桩与洗车",
        "攒钱记账花呗信用卡还款与消费降级", "小区物业收快递倒垃圾邻里日常小事",
    ]),
    ("tc_acg_meme", "zh-TW", "spoken", 0.12, [
        "二次元手遊抽卡大保底歪卡存石頭與強度黨討論", "遊戲配隊聖遺物詞條打深淵副本與抄作業",
        "當季動畫新番追番劇透吐槽與作畫崩壞評論", "漫展排隊買周邊立牌徽章痛包與同人本",
        "VTuber直播跟台剪輯烤肉連動與超話互動", "單機主機RPG開放世界探索解謎與速通",
        "電競MOBA排位連敗隊友掛機打野抓人吐槽", "FPS射擊槍法身法開麥報點與外掛抱怨",
        "音遊打歌全連手速譜面與獨立遊戲推薦", "Threads與Dcard最新流行語網路熱梗接話",
        "社群抽象吐槽社死瞬間情緒價值與破防日常", "Cosplay準備假髮道具外拍約攝影師聊天",
    ]),
    ("sc_acg_meme", "zh-CN", "spoken", 0.12, [
        "二游抽卡吃满大保底歪常驻屯抽与版本前瞻", "刷副本词条养成配队凹深渊与抄攻略作业",
        "B站追新番弹幕玩梗剧情反转与烂尾吐槽", "漫展逛摊买谷子吧唧立牌吃谷拼团与痛包",
        "虚拟主播直播间连麦切片二创与粉丝群聊天", "Steam单机肉鸽魂系动作游戏联机开黑吐槽",
        "MOBA排位连跪对线打野团战与队友甩锅", "FPS战术报点压枪手感开黑语音与外挂吐槽",
        "抖音小红书贴吧2024到2026最新热梗接话", "打工人抽象整活发疯文学班味已读乱回聊天",
        "网游公会打团本拍卖分金与跨服社交日常", "轻小说同人二创追更催更与声优配音讨论",
    ]),
    ("tc_written", "zh-TW", "written", 0.11, [
        "大語言模型提示詞工程微調量化與端側部署", "Rust與Swift系統開發記憶體安全效能調優",
        "前後端架構微服務容器化資料庫索引與快取", "資訊安全加密驗證隱私保護與系統可觀測性",
        "企業財報毛利率ETF指數投資與總體經濟分析", "房貸利率稅務申報保險規劃與個人資產配置",
        "臨床門診用藥檢驗報告慢性病追蹤與疫苗保健", "勞動法規合約條款智慧財產權與網購消保維權",
        "職場專案排期跨部門對齊簡報復盤與管理溝通", "數碼3C筆電手機相機鏡頭評測與客製化鍵盤",
        "電動車續航充電規格智慧家電與半導體製程", "航空訂票簽證保險海外自駕與飯店住宿攻略",
    ]),
    ("sc_written", "zh-CN", "written", 0.11, [
        "大模型提示词工程微调蒸馏量化与端侧推理", "Rust和Swift系统编程并发优化与内存安全",
        "全栈架构微服务容器编排数据库索引与高并发", "信息安全权限风控加密传输与日志监控排障",
        "财报分析估值指标基金ETF配置与宏观利率", "房贷政策个税专项扣除商业保险与家庭理财",
        "医院门诊用药体检指标解读慢性病管理与康复", "劳动仲裁合同违约知识产权保护与电商维权",
        "职场项目复盘需求评审进度对齐与汇报沟通", "数码3C手机电脑摄影器材测评与外设搭配",
        "新能源汽车智驾快充具身智能与芯片产业动态", "差旅报销机票酒店预订出境游签证与交通指南",
    ]),
    ("tc_disambig", "zh-TW", "spoken", 0.05, [
        "高頻句首自然銜接（幫我、麻煩、請問、剛才、明天、下班後、本來、結果、順便、幸好、乾脆）日常對話",
        "同音易錯字語境消歧（的/得/地、在/再、做/作、帶/戴、已讀/一讀、登入/登錄、介面/界面、帳/賬）自然短句",
        "易混淆詞語上下文對比（超時/超市、精度/京都、補丁/布丁、訪問/防蚊、部署/步數、權利/權力、必須/必需）",
        "高頻動賓與量詞固定搭配（確認一下、看一看、發給你、傳給我、點進去、切過去、存下來、清乾淨）生活口語",
    ]),
    ("sc_disambig", "zh-CN", "spoken", 0.05, [
        "高频句首自然衔接（帮我、麻烦、请问、刚才、明天、下班后、本来、结果、顺便、幸好、干脆）日常对话",
        "同音易错字语境消歧（的/得/地、在/再、做/作、带/戴、已读/一读、登录/登入、界面/介面、账/帐）自然短句",
        "易混淆词语上下文对比（超时/超市、精度/京都、补丁/布丁、访问/防蚊、部署/步数、权利/权力、必须/必需）",
        "高频动宾与量词固定搭配（确认一下、看一眼、发给你、传给我、点进去、切过去、存下来、清干净）生活口语",
    ]),
]

ANGLES_TW = [
    "兩人日常私訊問答與接話", "三人以上群組熱鬧討論與吐槽", "晚上睡前滑手機隨口閒聊",
    "中午午休一邊吃飯一邊分享近況", "剛遇到小狀況急著找人求救或問意見", "踩雷抱怨與給朋友避坑建議",
    "真心推薦好物或分享實用小技巧", "比較兩個選項猶豫不決請人幫挑", "回顧今天發生的趣事或尷尬瞬間",
    "計畫週末或下週行程確認細節", "線上詢問客服店員或賣家細節", "專業人士用白話向新手耐心解釋",
    "老手之間互相交流進階心得與細節", "預算有限精打細算討論性價比", "臨時改變計畫緊急溝通調整時間",
    "下班放空療癒分享與互相打氣", "從零開始入坑踩坑與最新心得", "細節控討論規格參數與實際體感",
]
ANGLES_CN = [
    "两人日常私信问答与自然接话", "多人微信群热闹讨论与互相吐槽", "晚上睡前刷手机随口闲聊",
    "中午午休边吃饭边分享近况", "刚遇到小状况急着找人求助问意见", "踩雷抱怨与给朋友避坑提醒",
    "真心安利好物或分享实用小技巧", "对比两个选项纠结请朋友帮忙挑", "复盘今天遇到的趣事或社死瞬间",
    "规划周末或下周行程核对细节", "线上咨询客服店家或卖家细节", "专业人士用大白话给新手讲解",
    "老玩家老手互相交流进阶心得", "预算有限精打细算讨论性价比", "临时改计划紧急沟通调整安排",
    "下班放空解压分享与互相鼓励", "从零入坑新手体验与最新感受", "细节控讨论规格参数与真实体感",
]

HAN = re.compile(r"[㐀-䶿一-鿿]")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")
BAD_TAIL = re.compile(r"(?:就可以了|就好了|就行了|真不錯|真不错|太好了|挺好的|很不錯|很不错|沒問題|没问题)$")


def ssl_ctx() -> ssl.SSLContext:
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except Exception:
        return ssl.create_default_context()


def access_token() -> str:
    return subprocess.run(
        ["gcloud", "auth", "print-access-token", "--quiet"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()


def build_requests(n_reqs: int, lines_per_req: int, max_out: int, seed: int) -> list[dict]:
    rng = random.Random(seed)
    counts = []
    used = 0
    for idx, item in enumerate(CATALOG):
        c = n_reqs - used if idx == len(CATALOG) - 1 else round(n_reqs * item[3])
        counts.append(c)
        used += c

    reqs = []
    seq = 0
    for (tag, script, reg, _w, topics), cnt in zip(CATALOG, counts):
        angles = ANGLES_TW if script == "zh-TW" else ANGLES_CN
        sys_msg = SYS_TW if script == "zh-TW" else SYS_CN
        style_hint = (
            ("簡短口語、自然接話" if reg == "spoken" else "語句完整、搭配精準")
            if script == "zh-TW"
            else ("简短口语、自然接话" if reg == "spoken" else "语句完整、搭配精准")
        )
        for i in range(cnt):
            seq += 1
            topic = topics[i % len(topics)]
            angle = angles[(i // len(topics) + i * 7) % len(angles)]
            salt = f"{rng.randrange(1000, 9999)}"
            if script == "zh-TW":
                user = (
                    f"場景：{topic}｜視角：{angle}（風格：{style_hint}；批次#{salt}）。"
                    f"直接輸出{lines_per_req}句互不重複的自然中文句子，每行一句10-24字，涵蓋多樣主語與動詞。"
                )
            else:
                user = (
                    f"场景：{topic}｜视角：{angle}（风格：{style_hint}；批次#{salt}）。"
                    f"直接输出{lines_per_req}句互不重复的自然中文句子，每行一句10-24字，涵盖多样主语与动词。"
                )
            reqs.append({
                "id": f"{tag}_{seq:05d}",
                "request": {
                    "systemInstruction": {"parts": [{"text": sys_msg}]},
                    "contents": [{"role": "user", "parts": [{"text": user}]}],
                    "generationConfig": {
                        "temperature": 0.92,
                        "maxOutputTokens": max_out,
                        "thinkingConfig": {"thinkingLevel": "LOW"},
                    },
                },
            })
    rng.shuffle(reqs)
    return reqs


def cost_estimate(n_reqs: int, avg_in: int, avg_out: int, max_out: int) -> dict:
    in_usd = n_reqs * avg_in * BATCH_IN_PER_M / 1e6
    exp_out_usd = n_reqs * avg_out * BATCH_OUT_PER_M / 1e6
    max_out_usd = n_reqs * max_out * BATCH_OUT_PER_M / 1e6
    return {
        "n_reqs": n_reqs,
        "expected_usd": round(in_usd + exp_out_usd, 2),
        "worst_case_usd": round(in_usd + max_out_usd, 2),
        "in_usd": round(in_usd, 2),
        "exp_out_usd": round(exp_out_usd, 2),
    }


def cmd_build(args):
    if args.max_budget_usd <= 0 or args.max_budget_usd > HARD_CAP_USD:
        raise SystemExit(f"ABORT: --max-budget-usd must be in (0, {HARD_CAP_USD}]")
    per_req_max = (args.avg_in * BATCH_IN_PER_M + args.max_out * BATCH_OUT_PER_M) / 1e6
    per_req_exp = (args.avg_in * BATCH_IN_PER_M + args.avg_out * BATCH_OUT_PER_M) / 1e6
    if args.strict_worst_case:
        n_reqs = int(args.max_budget_usd / per_req_max)
    else:
        n_reqs = min(int(args.max_budget_usd / per_req_exp), int((args.max_budget_usd * 1.05) / per_req_max))
    if args.limit:
        n_reqs = min(n_reqs, args.limit)
    est = cost_estimate(n_reqs, args.avg_in, args.avg_out, args.max_out)
    if est["worst_case_usd"] > HARD_CAP_USD + 0.01 and args.strict_worst_case:
        raise SystemExit(f"ABORT: worst_case {est['worst_case_usd']} > {HARD_CAP_USD}")
    reqs = build_requests(n_reqs, args.lines_per_req, args.max_out, args.seed)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        for r in reqs:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    est["est_sentences"] = n_reqs * args.lines_per_req
    est["out"] = str(out)
    est["bytes"] = out.stat().st_size
    print(json.dumps(est, ensure_ascii=False, indent=2))


def cmd_submit(args):
    url = f"https://aiplatform.googleapis.com/v1/projects/{PROJECT}/locations/{LOCATION}/batchPredictionJobs"
    body = {
        "displayName": args.display_name,
        "model": MODEL,
        "inputConfig": {"instancesFormat": "jsonl", "gcsSource": {"uris": [args.input_gcs_uri]}},
        "outputConfig": {"predictionsFormat": "jsonl", "gcsDestination": {"outputUriPrefix": args.output_gcs_prefix}},
    }
    req = urllib.request.Request(
        url, data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {access_token()}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30, context=ssl_ctx()) as r:
        res = json.loads(r.read().decode())
    print(json.dumps({k: res.get(k) for k in ("name", "state", "model", "createTime", "outputInfo")}, indent=2))


def cmd_status(args):
    url = f"https://aiplatform.googleapis.com/v1/{args.job_name}"
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {access_token()}"})
    with urllib.request.urlopen(req, timeout=30, context=ssl_ctx()) as r:
        res = json.loads(r.read().decode())
    keys = ("name", "displayName", "state", "createTime", "startTime", "endTime", "completionStats", "outputInfo", "error")
    print(json.dumps({k: res.get(k) for k in keys if res.get(k) is not None}, indent=2, ensure_ascii=False))


def load_eval_5grams() -> set[str]:
    grams = set()
    for p in sorted((ROOT / "Tests/eval").glob("*.tsv")):
        for line in p.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) >= 2:
                s = cols[1].strip()
                for i in range(max(0, len(s) - 4)):
                    grams.add(s[i : i + 5])
    return grams


def cmd_ingest(args):
    eval_5 = load_eval_5grams()
    blocked_re = blocked_terms.counted()
    seen = set()
    for xp in sorted((ROOT / "DataSources/mimo-everyday").glob("*.txt.xz")):
        with lzma.open(xp, "rt", encoding="utf-8") as f:
            seen.update(f.read().splitlines())

    chat_lines, written_lines = [], []
    stats = {"reqs": 0, "ok_reqs": 0, "in_tok": 0, "out_tok": 0, "thought_tok": 0, "raw": 0, "kept": 0, "blocked_eval": 0, "dup_bad": 0}
    in_path = Path(args.input_dir)
    files = sorted(in_path.rglob("*.jsonl")) if in_path.is_dir() else [in_path]
    for fp in files:
        with open(fp, encoding="utf-8", errors="ignore") as f:
            for line in f:
                if not line.strip():
                    continue
                stats["reqs"] += 1
                obj = json.loads(line)
                rid = str(obj.get("id", ""))
                resp = obj.get("response") or {}
                u = resp.get("usageMetadata") or {}
                stats["in_tok"] += int(u.get("promptTokenCount") or 0)
                stats["out_tok"] += int(u.get("candidatesTokenCount") or 0)
                stats["thought_tok"] += int(u.get("thoughtsTokenCount") or 0)
                cands = resp.get("candidates") or []
                if not cands:
                    continue
                stats["ok_reqs"] += 1
                parts = (cands[0].get("content") or {}).get("parts") or []
                txt = "\n".join(p.get("text", "") for p in parts if isinstance(p, dict) and not p.get("thought"))
                is_written = "_written_" in rid
                for raw in txt.splitlines():
                    stats["raw"] += 1
                    s = LIST_PREFIX.sub("", raw).strip().strip("「」\"'")
                    if not (6 <= len(s) <= 42) or BROKEN.search(s) or len(HAN.findall(s)) < 5 or s in seen:
                        stats["dup_bad"] += 1
                        continue
                    if blocked_re.search(s) or any(s[i : i + 5] in eval_5 for i in range(max(0, len(s) - 4))):
                        stats["blocked_eval"] += 1
                        continue
                    seen.add(s)
                    stats["kept"] += 1
                    (written_lines if is_written else chat_lines).append(s)

    actual_usd = (stats["in_tok"] * BATCH_IN_PER_M + (stats["out_tok"] + stats["thought_tok"]) * BATCH_OUT_PER_M) / 1e6
    stats["actual_batch_usd"] = round(actual_usd, 3)
    stats["chat_kept"] = len(chat_lines)
    stats["written_kept"] = len(written_lines)
    if args.out_prefix:
        for suffix, lines in (("chat", chat_lines), ("written", written_lines)):
            if not lines:
                continue
            p = Path(f"{args.out_prefix}-{suffix}.txt.xz")
            p.parent.mkdir(parents=True, exist_ok=True)
            with lzma.open(p, "wt", encoding="utf-8", preset=6) as out:
                out.write("\n".join(lines) + "\n")
            stats[f"{suffix}_sha256"] = hashlib.sha256(p.read_bytes()).hexdigest()
            stats[f"{suffix}_path"] = str(p)
    print(json.dumps(stats, ensure_ascii=False, indent=2))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    pb = sub.add_parser("build")
    pb.add_argument("--max-budget-usd", type=float, default=96.0)
    pb.add_argument("--lines-per-req", type=int, default=260)
    pb.add_argument("--avg-in", type=int, default=145)
    pb.add_argument("--avg-out", type=int, default=4450)
    pb.add_argument("--max-out", type=int, default=4850)
    pb.add_argument("--strict-worst-case", action="store_true", default=True)
    pb.add_argument("--limit", type=int, default=0)
    pb.add_argument("--seed", type=int, default=20261123)
    pb.add_argument("--out", default=str(ROOT / ".build/vertex-hydrogen-batch/batch100.jsonl"))

    ps = sub.add_parser("submit")
    ps.add_argument("--display-name", default="meowpuut-hydrogen-batch100")
    ps.add_argument("--input-gcs-uri", required=True)
    ps.add_argument("--output-gcs-prefix", required=True)

    pst = sub.add_parser("status")
    pst.add_argument("--job-name", required=True)

    pi = sub.add_parser("ingest")
    pi.add_argument("--input-dir", required=True)
    pi.add_argument("--out-prefix", default="")

    args = ap.parse_args()
    {"build": cmd_build, "submit": cmd_submit, "status": cmd_status, "ingest": cmd_ingest}[args.cmd](args)


if __name__ == "__main__":
    main()
