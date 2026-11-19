#!/usr/bin/env python3
"""Evaluates Stage-2 (Bridged Trigram + Skip-1) & Stage-3 (INT8 TinyCharReranker) N-best reranking."""
import argparse
import ctypes
import importlib.util
import json
import lzma
import sys
from pathlib import Path

import torch
import torch.nn.functional as F

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import colab_corpus_mix as mix  # noqa: E402
import colab_train_reranker as rerank  # noqa: E402


class MPNCandidateInfo(ctypes.Structure):
    _fields_ = [
        ("kind", ctypes.c_uint32),
        ("score_kind", ctypes.c_uint32),
        ("consumed_bytes", ctypes.c_uint64),
        ("remaining_offset_bytes", ctypes.c_uint64),
        ("score", ctypes.c_double),
    ]


def eval_ranking_mod():
    spec = importlib.util.spec_from_file_location("eval_ranking", ROOT / "scripts/eval-ranking.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def load_stage2_tables(tables_dir: Path) -> tuple[dict[tuple[str, str, str], float], dict[tuple[str, str], float]]:
    tri, skip1 = {}, {}
    tp, sp = tables_dir / "trigram_bridge.tsv.xz", tables_dir / "skip1_bigram.tsv.xz"
    if tp.exists():
        with lzma.open(tp, "rt", encoding="utf-8") as f:
            for ln in f:
                p = ln.rstrip("\n").split("\t")
                if len(p) >= 4:
                    tri[(p[0], p[1], p[2])] = int(p[3]) / 64.0
    if sp.exists():
        with lzma.open(sp, "rt", encoding="utf-8") as f:
            for ln in f:
                p = ln.rstrip("\n").split("\t")
                if len(p) >= 3:
                    skip1[(p[0], p[1])] = int(p[2]) / 64.0
    return tri, skip1


def stage2_score(text: str, t2s: dict[int, str], word_logp: dict[str, float], max_len: int, tri: dict, skip1: dict) -> float:
    simp = text.translate(t2s)
    total = 0.0
    for run in mix.HAN_RUN.findall(simp):
        toks = ["<s>", *mix.segment_han_dp(run, word_logp, max_len), "</s>"]
        for i in range(len(toks) - 2):
            total += tri.get((toks[i], toks[i + 1], toks[i + 2]), 0.0)
            total += 0.5 * skip1.get((toks[i], toks[i + 2]), 0.0)
    return total


def load_int8_model(model_dir: Path, device: str):
    vocab = json.loads((model_dir / "vocab.json").read_text(encoding="utf-8"))
    model = rerank.TinyCharReranker(vocab_size=len(vocab))
    int8_path = model_dir / "reranker_int8.pt"
    if int8_path.exists():
        packed = torch.load(int8_path, map_location="cpu", weights_only=True)
        sd = {}
        for k, v in packed.items():
            sd[k] = (v[0].float() * v[1].float().unsqueeze(-1)) if isinstance(v, (tuple, list)) else v.float()
        sd.setdefault("head.weight", sd["tok_emb.weight"])
        model.load_state_dict(sd, strict=True)
    else:
        ckpt = torch.load(model_dir / "reranker_fp32.pt", map_location="cpu", weights_only=True)
        model.load_state_dict(ckpt["state_dict"], strict=True)
    return model.to(device).eval(), vocab


@torch.no_grad()
def batch_nn_scores(model, vocab: dict[str, int], cands: list[str], device: str) -> list[float]:
    if not cands:
        return []
    seqs, lengths = [], []
    max_len = rerank.MAX_SEQ_LEN
    for c in cands:
        ids = [rerank.BOS_ID] + [vocab.get(ch, rerank.UNK_ID) for ch in c[: max_len - 2]] + [rerank.EOS_ID]
        lengths.append(len(ids) - 1)
        ids.extend([rerank.PAD_ID] * (max_len - len(ids)))
        seqs.append(ids)
    t = torch.tensor(seqs, dtype=torch.long, device=device)
    logits = model(t[:, :-1])
    lp = F.log_softmax(logits, dim=-1).gather(-1, t[:, 1:].unsqueeze(-1)).squeeze(-1)
    out = []
    for idx, length in enumerate(lengths):
        out.append(float(lp[idx, :length].sum().item()))
    return out


def collect_nbest(er, engine, rows, top_k: int) -> list[dict]:
    core = engine.core
    core.mpn_candidate_info.restype = ctypes.c_int
    core.mpn_candidate_info.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(MPNCandidateInfo), ctypes.c_size_t)
    items = []
    for script, sentence, pinyin in rows:
        keys = pinyin.replace(" ", "")
        sess = engine.session(script)
        cands = []
        try:
            for ch in keys:
                core.mpn_key(sess, ord(ch))
            cnt = core.mpn_candidate_count(sess)
            info = MPNCandidateInfo()
            for i in range(cnt):
                txt = core.mpn_candidate(sess, i).decode("utf-8")
                ok = core.mpn_candidate_info(sess, i, ctypes.byref(info), ctypes.sizeof(info))
                sc = float(info.score) if ok else -float(i)
                consumed = int(info.consumed_bytes) if ok else core.mpn_candidate_consumed(sess, i)
                if consumed == len(keys) or i == 0:
                    cands.append((txt, sc))
                    if len(cands) >= top_k:
                        break
        finally:
            core.mpn_session_destroy(sess)
        items.append({"script": script, "gold": sentence, "cands": cands})
    return items


def tune_weights(items_dev, word_logp, t2s, max_len, tri, skip1, model, vocab, device: str):
    feats = []
    for item in items_dev:
        texts = [c[0] for c in item["cands"]]
        base = [c[1] for c in item["cands"]]
        s2 = [stage2_score(t, t2s, word_logp, max_len, tri, skip1) for t in texts]
        nn = batch_nn_scores(model, vocab, texts, device) if model is not None else [0.0] * len(texts)
        feats.append((item["gold"], texts, base, s2, nn))

    def score_pair(w2: float, w3: float) -> float:
        return sum(
            int(texts[max(range(len(texts)), key=lambda i: base[i] + w2 * s2[i] + w3 * nn[i])] == gold)
            for gold, texts, base, s2, nn in feats if texts
        ) / max(1, len(feats))

    best_beta = max((0.0, 0.05, 0.1, 0.2, 0.35, 0.5, 0.8), key=lambda b: (score_pair(b, 0.0), -b))
    best_gamma = max((0.0, 0.02, 0.05, 0.1, 0.15, 0.25, 0.4, 0.6), key=lambda g: (score_pair(0.0, g), -g)) if model else 0.0
    best_both = max(
        ((b, g) for b in (0.0, 0.1, 0.2, 0.35) for g in ((0.0, 0.05, 0.1, 0.15, 0.25, 0.4) if model else (0.0,))),
        key=lambda bg: (score_pair(*bg), -(bg[0] + bg[1])),
    )
    return best_beta, best_gamma, best_both


def eval_dataset(items: list[dict], word_logp, t2s, max_len, tri, skip1, model, vocab, device: str, beta: float, gamma: float, combo: tuple[float, float]):
    scores = []
    for item in items:
        cands = item["cands"]
        texts = [c[0] for c in cands]
        base = [c[1] for c in cands]
        s2 = [stage2_score(t, t2s, word_logp, max_len, tri, skip1) for t in texts] if (tri or skip1) else [0.0] * len(texts)
        nn = batch_nn_scores(model, vocab, texts, device) if model is not None else [0.0] * len(texts)
        scores.append((item["gold"], texts, base, s2, nn))

    def acc(w2: float, w3: float) -> float:
        hits = 0
        for gold, texts, base, s2, nn in scores:
            if not texts:
                continue
            best_i = max(range(len(texts)), key=lambda i: base[i] + w2 * s2[i] + w3 * nn[i])
            hits += int(texts[best_i] == gold)
        return 100.0 * hits / max(1, len(scores))

    oracle = 100.0 * sum(1 for g, texts, *_ in scores if g in texts) / max(1, len(scores))
    return {
        "n": len(scores), "bigram_top1": round(acc(0.0, 0.0), 2), "oracle_topk": round(oracle, 2),
        "plus_stage2": round(acc(beta, 0.0), 2), "plus_stage3": round(acc(0.0, gamma), 2),
        "plus_stage2_3": round(acc(combo[0], combo[1]), 2),
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--resources", required=True)
    ap.add_argument("--tables-dir", default=str(ROOT / ".build/colab-corpus/tables"))
    ap.add_argument("--model-dir", default=str(ROOT / ".build/colab-corpus/reranker"))
    ap.add_argument("--top-k", type=int, default=8)
    ap.add_argument("--beta", type=float, default=0.35)
    ap.add_argument("--gamma", type=float, default=0.25)
    ap.add_argument("--tune-on", default=None)
    ap.add_argument("--files", nargs="+", default=[
        str(ROOT / "Tests/eval/heldout-sentences.tsv"),
        str(ROOT / "Tests/eval/test2-sentences.tsv"),
        str(ROOT / "Tests/eval/modern-stress-sentences.tsv"),
    ])
    args = ap.parse_args()
    er = eval_ranking_mod()
    lib = er.default_lib()
    engine = er.Engine(er.load_core(lib), Path(args.resources), True, False)
    word_logp, t2s, max_len = mix.load_lexicon_maps(Path(args.resources))
    tri, skip1 = load_stage2_tables(Path(args.tables_dir))
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    model, vocab = load_int8_model(Path(args.model_dir), device) if (Path(args.model_dir) / "vocab.json").exists() else (None, {})
    beta, gamma, combo = args.beta, args.gamma, (args.beta, args.gamma)
    if args.tune_on:
        dev_items = collect_nbest(er, engine, er.read_sentences(args.tune_on), args.top_k)
        beta, gamma, combo = tune_weights(dev_items, word_logp, t2s, max_len, tri, skip1, model, vocab, device)
        print(f"tuned on {Path(args.tune_on).name}: beta={beta}, gamma={gamma}, combo={combo}")
    for fpath in args.files:
        rows = er.read_sentences(fpath)
        items = collect_nbest(er, engine, rows, args.top_k)
        res = eval_dataset(items, word_logp, t2s, max_len, tri, skip1, model, vocab, device, beta, gamma, combo)
        print(f"{Path(fpath).name}: {json.dumps(res)}")


if __name__ == "__main__":
    main()
