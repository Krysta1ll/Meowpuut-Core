#!/usr/bin/env python3
"""Vertex AI Gemini 3.1 Flash-Lite live synthesis with strict cumulative <= $24.50 USD budget cap."""
import argparse
import hashlib
import json
import lzma
import re
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import blocked_terms  # noqa: E402
import colab_lexicon_mine as mine  # noqa: E402
from vertex_prompts import build_term_prompts, corpus_prompt  # noqa: E402

VERTEX_PROJECT = "gen-lang-client-0285872710"
VERTEX_MODEL = "gemini-3.1-flash-lite"
INPUT_USD_PER_M = 0.25
OUTPUT_USD_PER_M = 1.50
HARD_CAP_USD = 24.50  # Strictly below cumulative $25.00 USD limit ($15 initial + $10 Wave-2).
DEFAULT_LEDGER = ROOT / ".build/colab-corpus/spend_ledger.json"

HAN = re.compile(r"[㐀-䶿一-鿿]")
HAN_WORD = re.compile(r"^[㐀-䶿一-鿿]{2,8}$")
BROKEN = re.compile(r"[\x00-\x1f\x7f]")
LIST_PREFIX = re.compile(r"^\s*(?:\d+[.)、:：]|[-*•])\s*")


def make_ssl_context() -> ssl.SSLContext:
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except Exception:
        pem = Path("/etc/ssl/cert.pem")
        return ssl.create_default_context(cafile=str(pem)) if pem.exists() else ssl.create_default_context()


def vertex_endpoint(project: str = VERTEX_PROJECT, model: str = VERTEX_MODEL) -> str:
    return (
        f"https://aiplatform.googleapis.com/v1/projects/{project}/locations/global/"
        f"publishers/google/models/{model}:generateContent"
    )


def compute_usage_cost_usd(usage: dict | None) -> tuple[int, int, int, float]:
    u = usage or {}
    in_tok = int(u.get("promptTokenCount") or 0)
    cand_tok = int(u.get("candidatesTokenCount") or 0)
    thought_tok = int(u.get("thoughtsTokenCount") or 0)
    usd = (in_tok * INPUT_USD_PER_M + (cand_tok + thought_tok) * OUTPUT_USD_PER_M) / 1_000_000.0
    return in_tok, cand_tok, thought_tok, usd


def build_payload(sys_msg: str, user_msg: str, max_out: int, temp: float = 0.85) -> dict:
    return {
        "systemInstruction": {"parts": [{"text": sys_msg}]},
        "contents": [{"role": "user", "parts": [{"text": user_msg}]}],
        "generationConfig": {
            "temperature": temp,
            "maxOutputTokens": max_out,
            "thinkingConfig": {"thinkingLevel": "MINIMAL"},
        },
    }


def extract_text_and_usage(resp: dict) -> tuple[str, tuple[int, int, int, float]]:
    cands = resp.get("candidates") or []
    text = ""
    if cands:
        parts = (cands[0].get("content") or {}).get("parts") or []
        text = "\n".join(p.get("text", "") for p in parts if isinstance(p, dict) and p.get("thought") is not True)
    return text, compute_usage_cost_usd(resp.get("usageMetadata"))


class BudgetLedger:
    """Thread-safe cumulative spend tracker & hard circuit breaker across all runs."""

    def __init__(self, path: Path, max_budget_usd: float):
        if max_budget_usd <= 0.0 or max_budget_usd > HARD_CAP_USD:
            raise SystemExit(f"ABORT: --max-budget-usd ({max_budget_usd:.2f}) must be in (0, {HARD_CAP_USD:.2f}]")
        self.path = path
        self.cap = max_budget_usd
        self.lock = threading.Lock()
        self.stop_event = threading.Event()
        if path.exists():
            self.data = json.loads(path.read_text(encoding="utf-8"))
        else:
            self.data = {"total_usd": 0.0, "requests": 0, "in_tokens": 0, "out_tokens": 0, "thought_tokens": 0}
        if self.data["total_usd"] >= self.cap:
            self.stop_event.set()

    def record(self, usage_tuple: tuple[int, int, int, float]) -> float:
        in_t, out_t, th_t, usd = usage_tuple
        with self.lock:
            self.data["requests"] += 1
            self.data["in_tokens"] += in_t
            self.data["out_tokens"] += out_t
            self.data["thought_tokens"] += th_t
            self.data["total_usd"] = round(self.data["total_usd"] + usd, 6)
            if self.data["total_usd"] >= self.cap:
                self.stop_event.set()
            return self.data["total_usd"]

    def save(self):
        with self.lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self.path.write_text(json.dumps(self.data, indent=2) + "\n", encoding="utf-8")


class VertexClient:
    def __init__(self, project: str = VERTEX_PROJECT, model: str = VERTEX_MODEL):
        self.url = vertex_endpoint(project, model)
        self.ssl_ctx = make_ssl_context()
        self._tok_lock = threading.Lock()
        self._tok = ""
        self._tok_ts = 0.0

    def token(self, force: bool = False) -> str:
        with self._tok_lock:
            if force or not self._tok or (time.monotonic() - self._tok_ts > 2100):
                self._tok = subprocess.run(
                    ["gcloud", "auth", "print-access-token"], capture_output=True, text=True, check=True
                ).stdout.strip()
                self._tok_ts = time.monotonic()
            return self._tok

    def generate(self, sys_msg: str, user_msg: str, max_out: int, temp: float = 0.85) -> tuple[str, tuple[int, int, int, float]]:
        body = json.dumps(build_payload(sys_msg, user_msg, max_out, temp), ensure_ascii=False).encode("utf-8")
        last_err = None
        for attempt in range(5):
            tok = self.token(force=(attempt > 0 and isinstance(last_err, urllib.error.HTTPError) and last_err.code in (401, 403)))
            req = urllib.request.Request(
                self.url, data=body,
                headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"}, method="POST",
            )
            try:
                with urllib.request.urlopen(req, timeout=45, context=self.ssl_ctx) as resp:
                    return extract_text_and_usage(json.loads(resp.read().decode("utf-8")))
            except urllib.error.HTTPError as e:
                last_err = e
                if e.code in (401, 403, 429, 500, 502, 503, 504):
                    time.sleep(min(20.0, (1.8 ** attempt) + 0.5))
                    continue
                raise RuntimeError(f"HTTP {e.code}: {e.read().decode('utf-8', 'ignore')[:240]}") from e
            except Exception as e:
                last_err = e
                time.sleep(min(15.0, (1.6 ** attempt) + 0.4))
        raise RuntimeError(f"Vertex retries exhausted: {last_err}")


def run_parallel_tasks(tasks: list[dict], out_path: Path, client: VertexClient, ledger: BudgetLedger, workers: int, mode: str):
    out_path.parent.mkdir(parents=True, exist_ok=True)
    done_ids = set()
    if out_path.exists():
        for line in out_path.read_text(encoding="utf-8", errors="ignore").splitlines():
            if line.strip():
                try:
                    done_ids.add(json.loads(line)["id"])
                except Exception:
                    pass
    pending = [t for t in tasks if t["id"] not in done_ids]
    print(f"[{mode}] total={len(tasks):,} done={len(done_ids):,} pending={len(pending):,} spent=${ledger.data['total_usd']:.4f}/{ledger.cap:.2f}", flush=True)
    if not pending or ledger.stop_event.is_set():
        return
    write_lock, t0, ok, err = threading.Lock(), time.monotonic(), 0, 0

    def work(task: dict):
        if ledger.stop_event.is_set():
            return None
        sys_m, usr_m, mx = (task["sys"], task["user"], 1500) if mode == "terms" else corpus_prompt(task)
        text, usage = client.generate(sys_m, usr_m, mx, temp=0.85)
        spent = ledger.record(usage)
        return {**{k: task[k] for k in ("id", "field", "script", "register") if k in task}, "text": text, "usage": usage}, spent

    with open(out_path, "a", encoding="utf-8") as f, ThreadPoolExecutor(max_workers=workers) as pool:
        for fut in as_completed([pool.submit(work, t) for t in pending]):
            try:
                res = fut.result()
                if res is None:
                    continue
                rec, spent = res
                with write_lock:
                    f.write(json.dumps(rec, ensure_ascii=False) + "\n")
                    ok += 1
                    if ok % 25 == 0:
                        f.flush()
                        ledger.save()
                    if ok % 200 == 0 or ok == len(pending):
                        dt = max(0.1, time.monotonic() - t0)
                        print(f"[{mode}] {ok}/{len(pending)} ({ok/dt:.1f} req/s) spent=${spent:.4f}/{ledger.cap:.2f}", flush=True)
            except Exception as exc:
                err += 1
                if err <= 5:
                    print(f"[{mode}] err: {exc}", flush=True)
        f.flush()
    ledger.save()
    print(f"[{mode}] done ok={ok} err={err} total_spent=${ledger.data['total_usd']:.4f}/{ledger.cap:.2f}", flush=True)


def cmd_smoke(args):
    ledger = BudgetLedger(Path(args.ledger), args.max_budget_usd)
    client = VertexClient(args.project, args.model)
    samples = [
        ("zh-TW", "請只輸出 2 句台灣繁體中文短句（每句 10~20 字），自然提到大模型與微調。"),
        ("zh-TW", "請只輸出 2 句台灣繁體聊天口語短句，提到抽卡大保底或鬆弛感。"),
        ("zh-CN", "请只输出 2 句中国大陆简体中文短句（每句 10~20 字），自然提到布洛芬与急性发烧。"),
        ("zh-CN", "请只输出 2 句中国大陆职场群聊短句，自然提到灰度发布或电子榨菜。"),
        ("zh-TW", "請只輸出 2 句台灣繁體中文短句，自然對比「病例」與「病歷」。"),
    ]
    for i, (sc, usr) in enumerate(samples, 1):
        if ledger.stop_event.is_set():
            break
        txt, usage = client.generate("你是簡潔的中文語料寫手，不要多餘解釋。", usr, 160)
        total = ledger.record(usage)
        first = txt.strip().splitlines()[0] if txt.strip() else ""
        print(f"smoke #{i} [{sc}] in={usage[0]} out={usage[1]} thought={usage[2]} cost=${usage[3]:.6f} cum=${total:.6f} | {first[:42]}")
    ledger.save()


def cmd_gen_terms(args):
    ledger = BudgetLedger(Path(args.ledger), args.max_budget_usd)
    client = VertexClient(args.project, args.model)
    prompts = build_term_prompts()
    raw_path = ROOT / ".build/colab-corpus/vertex_terms_raw.jsonl"
    run_parallel_tasks(prompts, raw_path, client, ledger, args.workers, "terms")

    raw_pairs, seen = [], set()
    for line in raw_path.read_text(encoding="utf-8", errors="ignore").splitlines():
        if not line.strip():
            continue
        obj = json.loads(line)
        field = obj["field"]
        for row in obj["text"].splitlines():
            cols = [re.sub(r"[\s0-9.\-•*]+", "", c) for c in row.split("\t") if c.strip()]
            if len(cols) < 2:
                continue
            t, s = cols[0], cols[1]
            if len(t) == len(s) and HAN_WORD.fullmatch(t) and HAN_WORD.fullmatch(s):
                if not blocked_terms.zh_hit(s) and not blocked_terms.drop_hit(s) and (t, s) not in seen:
                    seen.add((t, s))
                    raw_pairs.append((t, s, field))
    if args.converter and raw_pairs:
        t_map = mine.convert_lines([s for _, s, _ in raw_pairs], args.converter, traditional=True)
        s_map = mine.convert_lines([t for t, _, _ in raw_pairs], args.converter, traditional=False)
        cleaned, final_seen = [], set()
        for t, s, f in raw_pairs:
            nt, ns = (t_map[s] if t == s else t), (s_map[t] if t == s else s)
            if len(nt) == len(ns) and HAN_WORD.fullmatch(nt) and HAN_WORD.fullmatch(ns) and (nt, ns) not in final_seen:
                final_seen.add((nt, ns))
                cleaned.append((nt, ns, f))
        raw_pairs = cleaned

    out_tsv = Path(args.out)
    out_tsv.parent.mkdir(parents=True, exist_ok=True)
    lines = ["# Project-owned LLM-written modern & domain terms (Traditional, Simplified, Field)"]
    lines.extend(f"{t}\t{s}\t{f}" for t, s, f in sorted(raw_pairs))
    out_tsv.write_text("\n".join(lines) + "\n", encoding="utf-8")
    mine.write_seed_manifest({"llm_term_count": len(raw_pairs)})
    print(f"gen-terms: saved {len(raw_pairs):,} clean terms -> {out_tsv}")


def load_eval_exclusions(eval_dir: Path, t2s: dict[int, str], converter: str | None) -> set[str]:
    texts = []
    for p in sorted(eval_dir.glob("*.tsv")):
        for line in p.read_text(encoding="utf-8").splitlines():
            if line.strip() and not line.startswith("#") and len(cols := line.split("\t")) >= 2:
                texts.append(cols[1].strip())
    all_forms = set(texts)
    all_forms.update(t.translate(t2s) for t in texts)
    if converter and texts:
        all_forms.update(mine.convert_lines(texts, converter, traditional=False).values())
        all_forms.update(mine.convert_lines(texts, converter, traditional=True).values())
    grams = set()
    for s in all_forms:
        clean = "".join(HAN.findall(s))
        for i in range(max(0, len(clean) - 4)):
            grams.add(clean[i : i + 5])
    return grams


def cmd_gen_corpus(args):
    ledger = BudgetLedger(Path(args.ledger), args.max_budget_usd)
    client = VertexClient(args.project, args.model)
    tasks = [json.loads(ln) for ln in Path(args.tasks).read_text(encoding="utf-8").splitlines() if ln.strip()]
    if args.limit:
        tasks = tasks[: args.limit]
    run_parallel_tasks(tasks, Path(args.out), client, ledger, args.workers, "corpus")


def read_t2s_map(lex_dir: Path) -> dict[int, str]:
    best: dict[str, tuple[str, int]] = {}
    with open(lex_dir / "native-pinyin.tsv", "r", encoding="utf-8") as f:
        next(f)
        for ln in f:
            if ln == "# context\n":
                break
            p = ln.rstrip("\n").split("\t")
            if len(p) >= 5 and len(p[2]) == 1 and len(p[3]) == 1 and p[4].lstrip("-").isdigit() and best.get(p[2], ("", -9999))[1] < int(p[4]):
                best[p[2]] = (p[3], int(p[4]))
    return {ord(k): v for k, (v, _) in best.items() if k != v}


def cmd_ingest_corpus(args):
    t2s = read_t2s_map(mine.resolve_lexicon_dir(args.lexicon_dir))
    eval_grams = load_eval_exclusions(ROOT / "Tests/eval", t2s, args.converter)
    blocked_re, seen = blocked_terms.counted(), set()
    out_dir = Path(args.out_dir)
    chat_name = f"{args.prefix}-chat-{args.batch_num}.txt.xz"
    dom_name = f"{args.prefix}-domains-{args.batch_num}.txt.xz"
    for xp in sorted(out_dir.glob("*.txt.xz")) if out_dir.exists() else []:
        if xp.name not in (chat_name, dom_name):
            with lzma.open(xp, "rt", encoding="utf-8") as f:
                seen.update(f.read().splitlines())
    spoken, written, stats, reg_tw = [], [], {"raw": 0, "eval_or_blocked": 0, "dup_or_bad": 0}, {"spoken": 0, "written": 0}
    for ln in Path(args.raw).read_text(encoding="utf-8", errors="ignore").splitlines():
        if not ln.strip():
            continue
        obj = json.loads(ln)
        is_spoken, is_tw = (obj.get("register") == "spoken"), (obj.get("script") == "zh-TW")
        for raw in obj.get("text", "").splitlines():
            stats["raw"] += 1
            s = LIST_PREFIX.sub("", raw).strip().strip("「」\"'")
            if not (6 <= len(s) <= 40) or BROKEN.search(s) or len(HAN.findall(s)) < 4 or s in seen:
                stats["dup_or_bad"] += 1
                continue
            han_only = "".join(HAN.findall(s))
            if blocked_re.search(s) or any(x[i : i + 5] in eval_grams for x in (han_only, han_only.translate(t2s)) for i in range(max(0, len(x) - 4))):
                stats["eval_or_blocked"] += 1
                continue
            seen.add(s)
            (spoken if is_spoken else written).append(s)
            reg_tw["spoken" if is_spoken else "written"] += int(is_tw)

    out_dir.mkdir(parents=True, exist_ok=True)
    for fname, lines in ((chat_name, spoken), (dom_name, written)):
        with lzma.open(out_dir / fname, "wt", encoding="utf-8", preset=6) as f:
            f.write("\n".join(lines) + ("\n" if lines else ""))
    src_path = out_dir / "SOURCE.json"
    if src_path.exists():
        meta = json.loads(src_path.read_text(encoding="utf-8"))
        for fname in (chat_name, dom_name):
            meta.setdefault("files", {})[fname] = hashlib.sha256((out_dir / fname).read_bytes()).hexdigest()
        meta[f"batch_{args.batch_num}"] = {"written_date": "2026-11-19", chat_name[:-7]: len(spoken), dom_name[:-7]: len(written), "tw_counts": reg_tw, "stats": stats}
        src_path.write_text(json.dumps(meta, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"ingest-corpus: spoken={len(spoken):,} written={len(written):,} stats={stats}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("smoke", "gen-terms", "gen-corpus"):
        p = sub.add_parser(name)
        p.add_argument("--project", default=VERTEX_PROJECT)
        p.add_argument("--model", default=VERTEX_MODEL)
        p.add_argument("--max-budget-usd", type=float, default=13.50)
        p.add_argument("--ledger", default=str(DEFAULT_LEDGER))
        if name == "gen-terms":
            p.add_argument("--workers", type=int, default=16)
            p.add_argument("--converter", default=None)
            p.add_argument("--out", default=str(ROOT / "DataSources/mimo-domains/modern-llm-terms.tsv"))
        elif name == "gen-corpus":
            p.add_argument("--tasks", required=True)
            p.add_argument("--out", default=str(ROOT / ".build/colab-corpus/vertex_corpus_raw.jsonl"))
            p.add_argument("--workers", type=int, default=24)
            p.add_argument("--limit", type=int, default=0)
    pi = sub.add_parser("ingest-corpus")
    pi.add_argument("--raw", default=str(ROOT / ".build/colab-corpus/vertex_corpus_raw.jsonl"))
    pi.add_argument("--out-dir", default=str(ROOT / "DataSources/mimo-everyday"))
    pi.add_argument("--lexicon-dir", default=None)
    pi.add_argument("--converter", default=None)
    pi.add_argument("--prefix", default="modern")
    pi.add_argument("--batch-num", default="3")
    args = ap.parse_args()
    handlers = {
        "smoke": cmd_smoke, "gen-terms": cmd_gen_terms,
        "gen-corpus": cmd_gen_corpus, "ingest-corpus": cmd_ingest_corpus,
    }
    handlers[args.cmd](args)


if __name__ == "__main__":
    main()
