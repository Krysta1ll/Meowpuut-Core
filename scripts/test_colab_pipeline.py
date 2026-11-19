#!/usr/bin/env python3
"""Offline verification suite for Vertex Synth, Lexicon Mining, Pairing & Reranker Pipeline."""
import array
import json
import lzma
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import colab_corpus_mix  # noqa: E402
import colab_lexicon_mine  # noqa: E402
import colab_train_reranker  # noqa: E402
import mimo_pairs_ext  # noqa: E402
import mimo_trigrams  # noqa: E402
import vertex_live_synth  # noqa: E402
import vertex_prompts  # noqa: E402


def assert_line_limits():
    targets = [
        ROOT / "Tests/eval/modern-stress-sentences.tsv",
        ROOT / "scripts/colab_lexicon_mine.py",
        ROOT / "scripts/colab_corpus_mix.py",
        ROOT / "scripts/colab_train_reranker.py",
        ROOT / "scripts/vertex_live_synth.py",
        ROOT / "scripts/vertex_prompts.py",
        ROOT / "scripts/wave2_colloc_planner.py",
        ROOT / "scripts/mimo_pairs_ext.py",
        ROOT / "scripts/mimo_trigrams.py",
        ROOT / "scripts/eval-context-rerank.py",
        ROOT / "scripts/test_colab_pipeline.py",
        ROOT / "crates/meowpuut-core/src/trigrams.rs",
        ROOT / "crates/meowpuut-core/tests/trigrams_tests.rs",
        ROOT / "crates/meowpuut-ffi/tests/in_place.rs",
        ROOT / "notebooks/Meowpuut_Colab_Corpus_and_LM_Pipeline.ipynb",
    ]
    for p in targets:
        n = len(p.read_text(encoding="utf-8").splitlines())
        assert n <= 400, f"{p} exceeds 400 lines ({n})"
    print("OK: all created/modified pipeline & Rust files <= 400 lines")


def verify_notebook_and_py_compile():
    nb = json.loads((ROOT / "notebooks/Meowpuut_Colab_Corpus_and_LM_Pipeline.ipynb").read_text(encoding="utf-8"))
    assert nb["nbformat"] == 4 and len(nb["cells"]) >= 8
    assert "chinese-fineweb-edu" not in json.dumps(nb)
    py_files = list(ROOT.glob("scripts/vertex_*.py")) + [
        ROOT / "scripts/colab_lexicon_mine.py",
        ROOT / "scripts/colab_corpus_mix.py",
        ROOT / "scripts/colab_train_reranker.py",
        ROOT / "scripts/wave2_colloc_planner.py",
        ROOT / "scripts/mimo_pairs_ext.py",
        ROOT / "scripts/mimo_pairs.py",
        ROOT / "scripts/mimo_trigrams.py",
        ROOT / "scripts/eval-context-rerank.py",
        ROOT / "scripts/eval-ranking.py",
    ]
    for pf in py_files:
        subprocess.run([sys.executable, "-m", "py_compile", str(pf)], check=True)
    print("OK: nbformat=4 clean JSON & py_compile passed")


def verify_vertex_budget_and_ingest():
    payload = vertex_live_synth.build_payload("sys", "user", 500)
    assert payload["generationConfig"]["thinkingConfig"]["thinkingLevel"] == "MINIMAL"
    assert len(vertex_prompts.build_term_prompts()) >= 180

    in_t, cand_t, th_t, usd = vertex_live_synth.compute_usage_cost_usd({
        "promptTokenCount": 1_000_000, "candidatesTokenCount": 500_000, "thoughtsTokenCount": 500_000,
    })
    assert (in_t, cand_t, th_t) == (1_000_000, 500_000, 500_000) and abs(usd - 1.75) < 1e-9

    with tempfile.TemporaryDirectory() as tmp:
        td = Path(tmp)
        try:
            vertex_live_synth.BudgetLedger(td / "bad.json", 25.0)
            raise AssertionError("expected abort for > 24.50 hard cap")
        except SystemExit:
            pass
        ledger = vertex_live_synth.BudgetLedger(td / "ok.json", 0.01)
        assert not ledger.stop_event.is_set()
        spent = ledger.record((40_000, 1_000, 0, 0.0115))
        assert spent >= 0.01 and ledger.stop_event.is_set()

        eval_grams = vertex_live_synth.load_eval_exclusions(ROOT / "Tests/eval", {ord("開"): "开", ord("發"): "发"}, None)
        assert "我们正在本" in eval_grams
        raw_f = td / "raw.jsonl"
        out_dir = td / "out"
        rows = [
            {"id": "1", "script": "zh-CN", "register": "written", "text": "我们正在本地部署开源大模型\n我们把向量数据库接入推理引擎"},
            {"id": "2", "script": "zh-TW", "register": "spoken", "text": "1. 醫生提醒發燒時先吃一顆布洛芬\n- 不管什麼都不該放行"},
        ]
        raw_f.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n", encoding="utf-8")
        subprocess.run([
            sys.executable, str(ROOT / "scripts/vertex_live_synth.py"), "ingest-corpus",
            "--raw", str(raw_f), "--out-dir", str(out_dir),
            "--lexicon-dir", str(colab_lexicon_mine.resolve_lexicon_dir(None)),
        ], check=True)
        w_txt = lzma.open(out_dir / "modern-domains-3.txt.xz", "rt", encoding="utf-8").read()
        s_txt = lzma.open(out_dir / "modern-chat-3.txt.xz", "rt", encoding="utf-8").read()
        assert "我们正在本地部署开源大模型" not in w_txt
        assert "向量数据库接入推理引擎" in w_txt and "布洛芬" in s_txt
    print("OK: Vertex MINIMAL config, cost math, circuit breaker & holdout ingest verified")


def verify_mimo_pairs_ext_and_trigrams():
    with tempfile.TemporaryDirectory() as tmp:
        tsv = Path(tmp) / "eval.tsv"
        tsv.write_text("# comment\nt\t測試一句繁體句子\tce shi\n", encoding="utf-8")
        assert mimo_pairs_ext.tsv_texts(tsv) == ["測試一句繁體句子"]
    uni, (codes, cnts) = mimo_pairs_ext.merge_weighted_sources([
        (0.25, array.array("Q", [0, 0, 4, 8]), array.array("Q", [(2 << 20) | 3]), array.array("Q", [4])),
        (2.0, array.array("Q", [0, 0, 1, 2]), array.array("Q", [(2 << 20) | 3]), array.array("Q", [3])),
    ], 4)
    assert uni == [0, 0, 3, 6] and list(codes) == [(2 << 20) | 3] and list(cnts) == [7]

    lex_dir = colab_lexicon_mine.resolve_lexicon_dir(None)
    with tempfile.TemporaryDirectory() as tmp:
        td = Path(tmp)
        shards = td / "shards"
        shards.mkdir()
        sample_lines = [
            "她把这台电脑的模型提示词改得很清楚",
            "她把这台电脑的模型提示词改得更准确",
            "她把这台电脑的模型提示词改了一下",
            "我们正在本地部署开源大模型",
        ] * 2
        with lzma.open(shards / "sample.txt.xz", "wt", encoding="utf-8") as f:
            f.write("\n".join(sample_lines) + "\n")
        tables_dir = td / "tables"
        subprocess.run([
            sys.executable, str(ROOT / "scripts/colab_corpus_mix.py"), "build-context-tables",
            "--lexicon-dir", str(lex_dir), "--corpus-dirs", str(shards),
            "--out-dir", str(tables_dir), "--min-trigram-count", "2",
        ], check=True)
        tri = lzma.open(tables_dir / "trigram_bridge.tsv.xz", "rt", encoding="utf-8").read()
        skip1 = lzma.open(tables_dir / "skip1_bigram.tsv.xz", "rt", encoding="utf-8").read()
        pairs = lzma.open(tables_dir / "rebalanced_pairs.txt.xz", "rt", encoding="utf-8").read()
        assert tri.strip() and skip1.strip() and "提示词" in pairs
        assert "部署\t大模型" not in pairs
        tri_bin = td / "mimo_trigrams.bin"
        subprocess.run([
            sys.executable, str(ROOT / "scripts/mimo_trigrams.py"), "pack",
            "--lexicon-dir", str(lex_dir),
            "--trigrams", str(tables_dir / "trigram_bridge.tsv.xz"),
            "--skip1", str(tables_dir / "skip1_bigram.tsv.xz"),
            "--out", str(tri_bin), "--tri-weight", "0.25", "--skip-weight", "0.15",
        ], check=True)
        assert tri_bin.read_bytes()[:12] == mimo_trigrams.MAGIC
        tq8, sq8 = mimo_trigrams.reweight_image(tri_bin, 0.30, 0.10)
        assert (tq8, sq8) == (77, 26)
    print("OK: mimo_pairs_ext & mimo_trigrams pack/reweight verified")


def verify_reranker_smoke():
    with tempfile.TemporaryDirectory() as tmp:
        td = Path(tmp)
        subprocess.run(
            [sys.executable, str(ROOT / "scripts/colab_train_reranker.py"), "smoke-test", "--out-dir", str(td)],
            check=True,
        )
        assert (td / "reranker_int8.pt").exists() and (td / "vocab.json").exists()
    print("OK: PyTorch INT8 reranker smoke-test verified")


def main():
    assert_line_limits()
    verify_notebook_and_py_compile()
    verify_vertex_budget_and_ingest()
    verify_mimo_pairs_ext_and_trigrams()
    verify_reranker_smoke()
    print("ALL PIPELINE SELF-TESTS PASSED!")


if __name__ == "__main__":
    main()
