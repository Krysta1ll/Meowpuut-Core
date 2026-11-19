#!/usr/bin/env python3
"""Anchor-Forced Gemini 3.1 Flash-Lite Batch Corpus Synthesis (HARD CAP <= $15.00 USD).

Subcommands:
  plan           Computes exact token/cost budget from anchor_batches.jsonl.
  build-jsonl    Builds Vertex AI or AI Studio Batch JSONL (aborts if > budget cap).
  submit-vertex  Submits a Vertex AI batchPredictionJobs job over GCS URIs.
  poll-vertex    Polls a Vertex AI batchPredictionJobs resource until completion.
  ingest         Cleans & deduplicates Batch JSONL output into DataSources/mimo-everyday/*.txt.xz.
"""
import argparse
import json
import lzma
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402

# Official Gemini 3.1 Flash-Lite Batch API rates (50% off standard $0.25 / $1.50 per 1M tokens)
BATCH_INPUT_USD_PER_M = 0.125
BATCH_OUTPUT_USD_PER_M = 0.75
HARD_BUDGET_CAP_USD = 15.0
DEFAULT_MODEL_VERTEX = "publishers/google/models/gemini-3.1-flash-lite"
DEFAULT_MAX_OUTPUT_TOKENS = 720

HAN = re.compile(r"[㐀-䶿一-鿿]")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")


def estimate_batch_cost_usd(
    num_reqs: int,
    avg_input_tokens: int,
    max_output_tokens: int,
    expected_fill_ratio: float = 0.70,
) -> tuple[float, float]:
    """Returns (expected_usd, worst_case_usd) for `num_reqs` Batch requests."""
    in_cost = (num_reqs * avg_input_tokens / 1_000_000.0) * BATCH_INPUT_USD_PER_M
    out_worst = (num_reqs * max_output_tokens / 1_000_000.0) * BATCH_OUTPUT_USD_PER_M
    out_exp = out_worst * expected_fill_ratio
    return (in_cost + out_exp, in_cost + out_worst)


def enforce_budget_cap(
    num_reqs: int,
    avg_input_tokens: int,
    max_output_tokens: int,
    requested_max_usd: float,
) -> tuple[float, float, float]:
    """Enforces `<= min(requested_max_usd, HARD_BUDGET_CAP_USD)` or aborts immediately."""
    if requested_max_usd <= 0.0 or requested_max_usd > HARD_BUDGET_CAP_USD:
        raise SystemExit(
            f"ABORT: --max-budget-usd ({requested_max_usd:.2f}) must be in (0, {HARD_BUDGET_CAP_USD:.2f}] USD"
        )
    effective_cap = min(requested_max_usd, HARD_BUDGET_CAP_USD)
    exp_usd, worst_usd = estimate_batch_cost_usd(num_reqs, avg_input_tokens, max_output_tokens)
    if worst_usd > effective_cap:
        raise SystemExit(
            f"ABORT: worst-case cost ${worst_usd:.2f} USD for {num_reqs} requests exceeds cap ${effective_cap:.2f} USD"
        )
    return (effective_cap, exp_usd, worst_usd)


def build_prompt(batch: dict, sentences_per_req: int) -> tuple[str, str]:
    script_rule = (
        "全句使用自然台灣繁體中文與台灣常見用語"
        if batch["script"] == "zh-TW"
        else "全句使用自然中国大陆简体中文与常见口语书面表达"
    )
    reg_rule = (
        "模擬真實QQ/微信/Thread社群群聊與朋友吐槽對話（短句、自然、生活化）"
        if batch.get("register") == "spoken"
        else "模擬專業討論、科技/財經/醫療/法律說明與高品質網路評論（語法完整、動賓搭配豐富）"
    )
    sys_msg = (
        f"你是中文輸入法語料編寫員。{script_rule}；{reg_rule}。\n"
        f"請輸出正好 {sentences_per_req} 行獨立中文句子（每行 10～36 字，絕不加序號、引號、分類標題或英文解釋；禁敏感與人身攻擊）。"
    )
    anchors = "、".join(batch["anchors"])
    user_msg = (
        f"領域：{batch['field']}\n"
        f"必含錨點詞（這 {sentences_per_req} 句請自然穿插用完所有錨點詞，展示多樣動詞／量詞／形容詞前後搭配）：{anchors}"
    )
    return (sys_msg, user_msg)


def format_batch_record(batch: dict, backend: str, max_out_tokens: int, sentences_per_req: int) -> dict:
    sys_msg, user_msg = build_prompt(batch, sentences_per_req)
    req_body = {
        "systemInstruction": {"parts": [{"text": sys_msg}]},
        "contents": [{"role": "user", "parts": [{"text": user_msg}]}],
        "generationConfig": {"temperature": 0.85, "maxOutputTokens": max_out_tokens},
    }
    if backend == "vertex":
        return {"request": req_body}
    return {"key": batch["id"], "request": req_body}


def load_batches(path: Path, repeat: int = 1, limit: int = 0) -> list[dict]:
    base = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    out = []
    for r in range(repeat):
        for item in base:
            cp = dict(item)
            cp["id"] = f"{item['id']}_r{r}" if r > 0 else item["id"]
            out.append(cp)
            if limit and len(out) >= limit:
                return out
    return out


def avg_prompt_tokens(batches: list[dict], sentences_per_req: int) -> int:
    if not batches:
        return 120
    sample = batches[: min(50, len(batches))]
    total_chars = sum(len(s) + len(u) for b in sample for s, u in [build_prompt(b, sentences_per_req)])
    return max(90, int((total_chars / len(sample)) * 1.15) + 12)


def cmd_plan(args):
    if args.max_budget_usd > HARD_BUDGET_CAP_USD:
        raise SystemExit(f"ABORT: --max-budget-usd must not exceed ${HARD_BUDGET_CAP_USD:.2f}")
    batches = load_batches(Path(args.anchors), repeat=args.repeat)
    avg_in = avg_prompt_tokens(batches, args.sentences_per_req)
    per_req_worst = estimate_batch_cost_usd(1, avg_in, args.max_output_tokens)[1]
    safe_cap_reqs = int(min(args.max_budget_usd, HARD_BUDGET_CAP_USD) / max(per_req_worst, 1e-9))
    planned = min(len(batches), safe_cap_reqs)
    exp_usd, worst_usd = estimate_batch_cost_usd(planned, avg_in, args.max_output_tokens)
    est_chars = planned * args.sentences_per_req * 22
    print(json.dumps({
        "available_batches": len(batches),
        "planned_requests": planned,
        "avg_input_tokens_per_req": avg_in,
        "max_output_tokens_per_req": args.max_output_tokens,
        "expected_cost_usd": round(exp_usd, 3),
        "worst_case_cost_usd": round(worst_usd, 3),
        "budget_cap_usd": round(min(args.max_budget_usd, HARD_BUDGET_CAP_USD), 2),
        "estimated_output_sentences": planned * args.sentences_per_req,
        "estimated_output_chars": est_chars,
    }, ensure_ascii=False, indent=2))


def cmd_build_jsonl(args):
    batches = load_batches(Path(args.anchors), repeat=args.repeat, limit=args.limit)
    avg_in = avg_prompt_tokens(batches, args.sentences_per_req)
    cap, exp_usd, worst_usd = enforce_budget_cap(len(batches), avg_in, args.max_output_tokens, args.max_budget_usd)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        for b in batches:
            rec = format_batch_record(b, args.backend, args.max_output_tokens, args.sentences_per_req)
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    print(
        f"build-jsonl [{args.backend}]: {len(batches)} reqs -> {out} | "
        f"expected=${exp_usd:.2f}, worst_case=${worst_usd:.2f} (cap=${cap:.2f})"
    )


def gcloud_access_token() -> str:
    return subprocess.run(
        ["gcloud", "auth", "print-access-token"], capture_output=True, text=True, check=True
    ).stdout.strip()


def cmd_submit_vertex(args):
    if args.max_budget_usd > HARD_BUDGET_CAP_USD:
        raise SystemExit(f"ABORT: --max-budget-usd exceeds ${HARD_BUDGET_CAP_USD:.2f}")
    if args.local_jsonl:
        n_reqs = sum(1 for line in Path(args.local_jsonl).read_text(encoding="utf-8").splitlines() if line.strip())
        enforce_budget_cap(n_reqs, 130, args.max_output_tokens, args.max_budget_usd)
    tok = gcloud_access_token()
    url = f"https://{args.location}-aiplatform.googleapis.com/v1/projects/{args.project}/locations/{args.location}/batchPredictionJobs"
    body = {
        "displayName": args.display_name,
        "model": args.model,
        "inputConfig": {"instancesFormat": "jsonl", "gcsSource": {"uris": [args.input_gcs_uri]}},
        "outputConfig": {"predictionsFormat": "jsonl", "gcsDestination": {"outputUriPrefix": args.output_gcs_prefix}},
    }
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        res = json.loads(resp.read().decode("utf-8"))
    print(f"submit-vertex: {res.get('name')} state={res.get('state')}")


def cmd_poll_vertex(args):
    url = f"https://{args.location}-aiplatform.googleapis.com/v1/{args.job_name}"
    while True:
        tok = gcloud_access_token()
        req = urllib.request.Request(url, headers={"Authorization": f"Bearer {tok}"})
        with urllib.request.urlopen(req, timeout=30) as resp:
            res = json.loads(resp.read().decode("utf-8"))
        state = res.get("state", "")
        print(f"poll-vertex: state={state}", flush=True)
        if state in ("JOB_STATE_SUCCEEDED", "JOB_STATE_FAILED", "JOB_STATE_CANCELLED"):
            if state != "JOB_STATE_SUCCEEDED":
                raise SystemExit(f"Vertex batch job ended in state={state}: {res.get('error')}")
            break
        time.sleep(args.interval_sec)


def load_eval_exclusions(eval_dir: Path) -> set[str]:
    grams = set()
    for p in sorted(eval_dir.glob("*.tsv")):
        for line in p.read_text(encoding="utf-8").splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) >= 2:
                s = cols[1].strip()
                for i in range(max(0, len(s) - 4)):
                    grams.add(s[i : i + 5])
    return grams


def has_eval_overlap(text: str, grams: set[str]) -> bool:
    return any(text[i : i + 5] in grams for i in range(max(0, len(text) - 4)))


def extract_response_text(obj: dict) -> str:
    resp = obj.get("response") or obj
    cands = resp.get("candidates") or []
    if not cands:
        return ""
    parts = (cands[0].get("content") or {}).get("parts") or []
    return "\n".join(p.get("text", "") for p in parts if isinstance(p, dict))


def cmd_ingest(args):
    eval_grams = load_eval_exclusions(ROOT / "Tests/eval")
    blocked_re = blocked_terms.counted()
    seen = set()
    for existing in ("everyday.txt.xz", "chat-2.txt.xz", "written-2.txt.xz", "mixed-2.txt.xz"):
        xp = ROOT / "DataSources/mimo-everyday" / existing
        if xp.exists():
            with lzma.open(xp, "rt", encoding="utf-8") as f:
                seen.update(f.read().splitlines())

    out_spoken, out_written = [], []
    stats = {"raw_lines": 0, "blocked_or_eval": 0, "dup_or_bad": 0}
    in_files = sorted(Path(args.input_dir).glob("*.jsonl")) if Path(args.input_dir).is_dir() else [Path(args.input_dir)]
    for fp in in_files:
        for line in fp.read_text(encoding="utf-8", errors="ignore").splitlines():
            if not line.strip():
                continue
            obj = json.loads(line)
            txt = extract_response_text(obj)
            is_spoken = any(k in json.dumps(obj, ensure_ascii=False) for k in ("熱梗", "ACG", "群聊", "吐槽"))
            for raw in txt.splitlines():
                stats["raw_lines"] += 1
                s = LIST_PREFIX.sub("", raw).strip().strip("「」\"'")
                if not (6 <= len(s) <= 40) or BROKEN.search(s) or len(HAN.findall(s)) < 4 or s in seen:
                    stats["dup_or_bad"] += 1
                    continue
                if blocked_re.search(s) or has_eval_overlap(s, eval_grams):
                    stats["blocked_or_eval"] += 1
                    continue
                seen.add(s)
                (out_spoken if is_spoken else out_written).append(s)

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    with lzma.open(out_dir / "modern-chat-3.txt.xz", "wt", encoding="utf-8", preset=6) as f:
        f.write("\n".join(out_spoken) + ("\n" if out_spoken else ""))
    with lzma.open(out_dir / "modern-domains-3.txt.xz", "wt", encoding="utf-8", preset=6) as f:
        f.write("\n".join(out_written) + ("\n" if out_written else ""))
    print(f"ingest: spoken={len(out_spoken):,}, written={len(out_written):,}, stats={stats}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("plan", "build-jsonl"):
        p = sub.add_parser(name)
        p.add_argument("--anchors", default=str(ROOT / ".build/colab-corpus/anchor_batches.jsonl"))
        p.add_argument("--max-budget-usd", type=float, default=12.0)
        p.add_argument("--max-output-tokens", type=int, default=DEFAULT_MAX_OUTPUT_TOKENS)
        p.add_argument("--sentences-per-req", type=int, default=16)
        p.add_argument("--repeat", type=int, default=1)
        if name == "build-jsonl":
            p.add_argument("--backend", choices=("vertex", "aistudio"), default="vertex")
            p.add_argument("--out", default=str(ROOT / ".build/colab-corpus/batch_requests.jsonl"))
            p.add_argument("--limit", type=int, default=0)

    ps = sub.add_parser("submit-vertex")
    ps.add_argument("--project", required=True)
    ps.add_argument("--location", default="us-central1")
    ps.add_argument("--model", default=DEFAULT_MODEL_VERTEX)
    ps.add_argument("--input-gcs-uri", required=True)
    ps.add_argument("--output-gcs-prefix", required=True)
    ps.add_argument("--display-name", default="meowpuut-corpus-synth")
    ps.add_argument("--local-jsonl", default=None)
    ps.add_argument("--max-budget-usd", type=float, default=12.0)
    ps.add_argument("--max-output-tokens", type=int, default=DEFAULT_MAX_OUTPUT_TOKENS)

    pp = sub.add_parser("poll-vertex")
    pp.add_argument("--location", default="us-central1")
    pp.add_argument("--job-name", required=True)
    pp.add_argument("--interval-sec", type=int, default=30)

    pi = sub.add_parser("ingest")
    pi.add_argument("--input-dir", required=True)
    pi.add_argument("--out-dir", default=str(ROOT / "DataSources/mimo-everyday"))

    args = ap.parse_args()
    {"plan": cmd_plan, "build-jsonl": cmd_build_jsonl, "submit-vertex": cmd_submit_vertex,
     "poll-vertex": cmd_poll_vertex, "ingest": cmd_ingest}[args.cmd](args)


if __name__ == "__main__":
    main()
