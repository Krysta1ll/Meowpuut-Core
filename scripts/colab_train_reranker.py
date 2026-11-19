#!/usr/bin/env python3
"""Colab Free T4/L4 GPU Tiny Character-Level Context Reranker (~2.7M params, ~3-4 MB INT8).

Scores candidate sentences conditioned on up to 24 chars of left document context via causal
cross-entropy (`log P(candidate | left_context)`), resolving long-range homophones that 2-gram tables miss.

Subcommands:
  train       Trains 4L d=192 Causal Transformer on `.txt`/`.txt.xz` shards + INT8 export.
  smoke-test  Runs a fast 2-step CPU training, INT8 quantization, and rerank sanity check.
  eval-rerank Evaluates Top-1 reranking accuracy on `left_ctx<TAB>c1|c2|...<TAB>gold` TSV.
"""
import argparse
import json
import lzma
import math
import random
from collections import Counter
from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import DataLoader, TensorDataset

PAD_ID, BOS_ID, EOS_ID, UNK_ID = 0, 1, 2, 3
SPECIAL_TOKENS = ["<pad>", "<bos>", "<eos>", "<unk>"]
MAX_SEQ_LEN = 48


class CausalSelfAttention(nn.Module):
    def __init__(self, d_model: int, n_heads: int, max_len: int):
        super().__init__()
        self.n_heads = n_heads
        self.head_dim = d_model // n_heads
        self.qkv = nn.Linear(d_model, d_model * 3, bias=False)
        self.proj = nn.Linear(d_model, d_model, bias=False)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        b, t, c = x.shape
        qkv = self.qkv(x).view(b, t, 3, self.n_heads, self.head_dim).permute(2, 0, 3, 1, 4)
        y = F.scaled_dot_product_attention(qkv[0], qkv[1], qkv[2], is_causal=True)
        return self.proj(y.transpose(1, 2).contiguous().view(b, t, c))


class TinyTransformerBlock(nn.Module):
    def __init__(self, d_model: int, n_heads: int, d_ff: int, max_len: int):
        super().__init__()
        self.ln1 = nn.LayerNorm(d_model)
        self.attn = CausalSelfAttention(d_model, n_heads, max_len)
        self.ln2 = nn.LayerNorm(d_model)
        self.ffn = nn.Sequential(
            nn.Linear(d_model, d_ff, bias=False),
            nn.GELU(),
            nn.Linear(d_ff, d_model, bias=False),
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = x + self.attn(self.ln1(x))
        return x + self.ffn(self.ln2(x))


class TinyCharReranker(nn.Module):
    """Weight-tied 4L d=192 Causal Char LM (~2.65M params with vocab=6500)."""

    def __init__(self, vocab_size: int, d_model: int = 192, n_layers: int = 4, n_heads: int = 4, d_ff: int = 512):
        super().__init__()
        self.tok_emb = nn.Embedding(vocab_size, d_model)
        self.pos_emb = nn.Embedding(MAX_SEQ_LEN, d_model)
        self.blocks = nn.ModuleList([TinyTransformerBlock(d_model, n_heads, d_ff, MAX_SEQ_LEN) for _ in range(n_layers)])
        self.ln_f = nn.LayerNorm(d_model)
        self.head = nn.Linear(d_model, vocab_size, bias=False)
        self.head.weight = self.tok_emb.weight

    def forward(self, idx: torch.Tensor) -> torch.Tensor:
        b, t = idx.shape
        pos = torch.arange(0, t, device=idx.device).unsqueeze(0)
        x = self.tok_emb(idx) + self.pos_emb(pos)
        for blk in self.blocks:
            x = blk(x)
        return self.head(self.ln_f(x))


def load_eval_grams() -> set[str]:
    eval_dir = Path(__file__).resolve().parent.parent / "Tests/eval"
    grams = set()
    for p in sorted(eval_dir.glob("*.tsv")) if eval_dir.exists() else []:
        for ln in p.read_text(encoding="utf-8").splitlines():
            if ln.strip() and not ln.startswith("#") and len(cols := ln.split("\t")) >= 2:
                s = cols[1].strip()
                for i in range(max(0, len(s) - 4)):
                    grams.add(s[i : i + 5])
    return grams


def read_corpus_lines(dirs: list[Path], max_lines: int = 1_500_000) -> list[str]:
    eval_grams = load_eval_grams()
    lines, seen = [], set()
    for d in dirs:
        paths = [d] if d.is_file() else sorted(list(d.glob("*.txt.xz")) + list(d.glob("*.txt")))
        for fp in paths:
            opener = lzma.open if fp.suffix == ".xz" else open
            with opener(fp, "rt", encoding="utf-8", errors="ignore") as f:
                for line in f:
                    s = line.strip()
                    if len(s) < 4 or s in seen:
                        continue
                    if any(s[i : i + 5] in eval_grams for i in range(max(0, len(s) - 4))):
                        continue
                    seen.add(s)
                    lines.append(s)
                    if len(lines) >= max_lines:
                        return lines
    return lines


def build_vocab(lines: list[str], max_vocab: int = 6500) -> dict[str, int]:
    cnt: Counter[str] = Counter()
    for line in lines:
        cnt.update(line)
    vocab = {tok: i for i, tok in enumerate(SPECIAL_TOKENS)}
    for ch, _ in cnt.most_common(max_vocab - len(SPECIAL_TOKENS)):
        if ch not in vocab:
            vocab[ch] = len(vocab)
    return vocab


def encode_lines(lines: list[str], vocab: dict[str, int], seq_len: int = MAX_SEQ_LEN) -> torch.Tensor:
    seqs = []
    for line in lines:
        ids = [BOS_ID] + [vocab.get(ch, UNK_ID) for ch in line[: seq_len - 2]] + [EOS_ID]
        if len(ids) < seq_len:
            ids.extend([PAD_ID] * (seq_len - len(ids)))
        seqs.append(ids)
    return torch.tensor(seqs, dtype=torch.long)


@torch.no_grad()
def score_candidate(model: nn.Module, vocab: dict[str, int], left_ctx: str, candidate: str, device: str = "cpu") -> float:
    """Computes log-probability sum of `candidate` conditioned on `left_ctx` (up to 24 chars)."""
    ctx = left_ctx[-24:]
    full = ctx + candidate
    ids = [BOS_ID] + [vocab.get(ch, UNK_ID) for ch in full[: MAX_SEQ_LEN - 2]] + [EOS_ID]
    x = torch.tensor([ids[:-1]], dtype=torch.long, device=device)
    y = torch.tensor([ids[1:]], dtype=torch.long, device=device)
    logits = model(x)
    log_probs = F.log_softmax(logits, dim=-1)
    tok_lp = log_probs.gather(-1, y.unsqueeze(-1)).squeeze(-1)[0]
    ctx_offset = len(ctx)
    end_idx = min(len(tok_lp), ctx_offset + len(candidate) + 1)
    return float(tok_lp[ctx_offset:end_idx].sum().item())


def quantize_dynamic_int8(model: nn.Module) -> nn.Module:
    cpu_model = model.to("cpu").eval()
    for eng in ("fbgemm", "qnnpack"):
        if eng in torch.backends.quantized.supported_engines:
            torch.backends.quantized.engine = eng
            return torch.ao.quantization.quantize_dynamic(cpu_model, {nn.Linear}, dtype=torch.qint8)
    return cpu_model


def export_int8_weight_blob(model: nn.Module, out_path: Path) -> float:
    """Packs Linear/Embedding weights into portable per-row symmetric INT8 + fp16 scales."""
    packed = {}
    for k, t in model.cpu().state_dict().items():
        if k == "head.weight":
            continue  # Weight-tied to tok_emb.weight
        if t.dim() == 2 and t.dtype in (torch.float32, torch.float16, torch.bfloat16):
            ft = t.float()
            scale = ft.abs().amax(dim=-1, keepdim=True).clamp(min=1e-6) / 127.0
            q = (ft / scale).round().clamp(-127, 127).to(torch.int8)
            packed[k] = (q, scale.squeeze(-1).half())
        else:
            packed[k] = t.half() if t.is_floating_point() else t
    torch.save(packed, out_path)
    return out_path.stat().st_size / 1e6


def export_artifacts(model: TinyCharReranker, vocab: dict[str, int], out_dir: Path):
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "vocab.json").write_text(json.dumps(vocab, ensure_ascii=False) + "\n", encoding="utf-8")
    fp32_path = out_dir / "reranker_fp32.pt"
    torch.save({"state_dict": model.cpu().state_dict(), "vocab_size": len(vocab)}, fp32_path)
    qmodel = quantize_dynamic_int8(model)
    int8_path = out_dir / "reranker_int8.pt"
    int8_mb = export_int8_weight_blob(model, int8_path)
    onnx_status = "skipped (install `onnx` in Colab to export .onnx)"
    try:
        import onnx  # noqa: F401
        dummy = torch.zeros(1, MAX_SEQ_LEN, dtype=torch.long)
        torch.onnx.export(model.cpu().eval(), dummy, str(out_dir / "reranker_fp32.onnx"), opset_version=17)
        onnx_status = f"exported ({(out_dir / 'reranker_fp32.onnx').stat().st_size / 1e6:.2f} MB)"
    except Exception as exc:
        onnx_status = f"onnx skipped ({exc})"
    params = sum(p.numel() for p in model.parameters())
    print(json.dumps({
        "params": params,
        "vocab_size": len(vocab),
        "fp32_mb": round(fp32_path.stat().st_size / 1e6, 2),
        "int8_pt_mb": round(int8_mb, 2),
        "onnx": onnx_status,
    }, ensure_ascii=False, indent=2))
    return qmodel


def train_loop(model: TinyCharReranker, tensor_data: torch.Tensor, epochs: int, batch_size: int, lr: float, device: str):
    import time
    loader = DataLoader(TensorDataset(tensor_data), batch_size=batch_size, shuffle=True, drop_last=len(tensor_data) > batch_size)
    opt = torch.optim.AdamW(model.parameters(), lr=lr, weight_decay=0.01)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, T_max=max(1, epochs * len(loader)))
    scaler = torch.amp.GradScaler("cuda") if device.startswith("cuda") else None
    amp_dev = "cuda" if device.startswith("cuda") else ("mps" if device == "mps" else None)
    amp_dtype = torch.float16 if amp_dev == "cuda" else torch.bfloat16

    model.to(device).train()
    for ep in range(1, epochs + 1):
        tot_loss, steps, t0 = 0.0, 0, time.monotonic()
        for (batch,) in loader:
            max_t = max(4, int((batch != PAD_ID).sum(dim=1).max().item()))
            batch = batch[:, :max_t].to(device)
            x, y = batch[:, :-1], batch[:, 1:]
            opt.zero_grad(set_to_none=True)
            if scaler is not None:
                with torch.amp.autocast("cuda", dtype=torch.float16):
                    logits = model(x)
                    loss = F.cross_entropy(logits.reshape(-1, logits.size(-1)), y.reshape(-1), ignore_index=PAD_ID)
                scaler.scale(loss).backward()
                scaler.step(opt)
                scaler.update()
            elif amp_dev == "mps":
                with torch.amp.autocast("mps", dtype=amp_dtype):
                    logits = model(x)
                    loss = F.cross_entropy(logits.reshape(-1, logits.size(-1)), y.reshape(-1), ignore_index=PAD_ID)
                loss.backward()
                opt.step()
            else:
                logits = model(x)
                loss = F.cross_entropy(logits.reshape(-1, logits.size(-1)), y.reshape(-1), ignore_index=PAD_ID)
                loss.backward()
                opt.step()
            sched.step()
            tot_loss += float(loss.item())
            steps += 1
            if steps % 200 == 0:
                dt = max(0.1, time.monotonic() - t0)
                print(f"epoch {ep}/{epochs} step {steps}/{len(loader)} loss={tot_loss/steps:.4f} ({steps/dt:.1f} step/s)", flush=True)
        print(f"epoch {ep}/{epochs} done avg_loss={tot_loss / max(1, steps):.4f} time={time.monotonic()-t0:.1f}s", flush=True)


def cmd_train(args):
    lines = read_corpus_lines([Path(d) for d in args.corpus_dirs], max_lines=args.max_lines)
    if len(lines) < 10:
        raise SystemExit("need at least 10 corpus lines for training")
    random.Random(42).shuffle(lines)
    vocab = build_vocab(lines, max_vocab=args.max_vocab)
    data = encode_lines(lines, vocab)
    device = args.device or ("cuda" if torch.cuda.is_available() else "cpu")
    model = TinyCharReranker(vocab_size=len(vocab))
    train_loop(model, data, args.epochs, args.batch_size, args.lr, device)
    export_artifacts(model, vocab, Path(args.out_dir))


def cmd_smoke_test(args):
    synth = [
        "我们正在本地部署开源大模型", "记得在系统提示词里加上约束", "医生给他开了两盒布洛芬缓释胶囊",
        "他因为连续发烧只能先吃一片布洛芬", "这个智能体能自动呼叫外部工具", "下班回家第一时间打开原神清体力",
        "作为标准非酋每次抽卡都得吃大保底", "高并发场景下必须控制锁的粒度", "她拍出来的旅游照片很有松弛感",
        "年轻人热衷买徽章带火了谷子经济", "网络抖动导致长连接偶尔断开重连", "检查一下反向代理的超时配置",
    ] * 8
    vocab = build_vocab(synth, max_vocab=512)
    data = encode_lines(synth, vocab)
    model = TinyCharReranker(vocab_size=len(vocab), d_model=192, n_layers=4, n_heads=4, d_ff=512)
    train_loop(model, data, epochs=2, batch_size=16, lr=1e-3, device="cpu")
    qmodel = export_artifacts(model, vocab, Path(args.out_dir))
    s_good = score_candidate(qmodel, vocab, "医生给他开了", "两盒布洛芬")
    assert math.isfinite(s_good)
    print(f"smoke-test OK: int8 score={s_good:.3f}")


def cmd_eval_rerank(args):
    out_dir = Path(args.model_dir)
    vocab = json.loads((out_dir / "vocab.json").read_text(encoding="utf-8"))
    ckpt = torch.load(out_dir / "reranker_fp32.pt", map_location="cpu", weights_only=True)
    model = TinyCharReranker(vocab_size=ckpt["vocab_size"])
    model.load_state_dict(ckpt["state_dict"])
    qmodel = quantize_dynamic_int8(model)

    rows = [ln.split("\t") for ln in Path(args.tsv).read_text(encoding="utf-8").splitlines() if ln.strip() and not ln.startswith("#")]
    base_hit = sum(1 for r in rows if r[1].split("|")[0] == r[2])
    rerank_hit = 0
    for left, cands_str, gold in rows:
        cands = cands_str.split("|")
        best = max(cands, key=lambda c: score_candidate(qmodel, vocab, left, c))
        rerank_hit += int(best == gold)
    n = max(1, len(rows))
    print(f"eval-rerank ({len(rows)} cases): baseline={base_hit/n:.1%}, rerank={rerank_hit/n:.1%}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    pt = sub.add_parser("train")
    pt.add_argument("--corpus-dirs", nargs="+", required=True)
    pt.add_argument("--out-dir", default=str(Path(__file__).resolve().parent.parent / ".build/reranker"))
    pt.add_argument("--max-lines", type=int, default=1_200_000)
    pt.add_argument("--max-vocab", type=int, default=6500)
    pt.add_argument("--epochs", type=int, default=3)
    pt.add_argument("--batch-size", type=int, default=256)
    pt.add_argument("--lr", type=float, default=6e-4)
    pt.add_argument("--device", default=None)

    ps = sub.add_parser("smoke-test")
    ps.add_argument("--out-dir", default=str(Path(__file__).resolve().parent.parent / ".build/reranker-smoke"))

    pe = sub.add_parser("eval-rerank")
    pe.add_argument("--model-dir", required=True)
    pe.add_argument("--tsv", required=True)

    args = ap.parse_args()
    {"train": cmd_train, "smoke-test": cmd_smoke_test, "eval-rerank": cmd_eval_rerank}[args.cmd](args)


if __name__ == "__main__":
    main()
