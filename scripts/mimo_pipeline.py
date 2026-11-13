"""Offline mimo lexicon pipeline: builds unified mimo-tc.json and mimo-sc.json from open-source corpora."""
import collections
import hashlib
import json
import math
import pathlib
import re
import subprocess
import unicodedata

ROOT = pathlib.Path(__file__).resolve().parent.parent
MCBOPOMOFO_BASE = "https://raw.githubusercontent.com/openvanilla/McBopomofoData/df084385e546287fc072dfce486d46f102ef9bd4/Data"
OPENCC_BASE = "https://raw.githubusercontent.com/BYVoid/OpenCC/master"
JIEBA_BASE = "https://raw.githubusercontent.com/fxsjy/jieba/master"
PINYIN_PRO_BASE = "https://raw.githubusercontent.com/zh-lx/pinyin-pro/0785234b4dc0fa3ea9f244e3d77703b2df6a9db4"

SMOOTHING_FLOOR_I16 = 2200
MAX_LOG_PROB_I16 = 10000


def clamp_i16_weight(weight: int | float) -> int:
    """Clamp calibrated log-probability weight into the positive i16 engine range [1, 10000]."""
    return max(1, min(MAX_LOG_PROB_I16, round(weight)))


def log_prob_weight(raw_freq: float, scale: float = 360.0, smoothing: int = 1) -> int:
    """Map raw corpus count/probability onto the unified i16 Log-Probability scale."""
    if raw_freq <= 0:
        return max(1, smoothing)
    return clamp_i16_weight(scale * math.log1p(raw_freq))


def _is_cjk_phrase(text: str, min_len: int = 2, max_len: int = 4) -> bool:
    return min_len <= len(text) <= max_len and all(0x3400 <= ord(ch) <= 0x9FFF for ch in text)


def _download_text(url: str) -> str:
    return subprocess.run(
        ["curl", "-fsSL", "--max-time", "90", url],
        text=True,
        capture_output=True,
        check=True,
    ).stdout


def numbered_pinyin(syllable: str) -> str:
    """Convert tone-marked UTF-8 pinyin syllable into tone-numbered ASCII (1..5)."""
    result, tone = "", "5"
    for char in unicodedata.normalize("NFD", syllable.lower()):
        if char in {"\u0304", "\u0301", "\u030c", "\u0300"}:
            tone = {"\u0304": "1", "\u0301": "2", "\u030c": "3", "\u0300": "4"}[char]
        elif char == "\u0308" and result.endswith("u"):
            result = result[:-1] + "v"
        elif char == "\u0302" and result.endswith("e"):
            result += "h"
        else:
            result += char
    return result + tone


def load_mimo_tc_dataset() -> dict:
    """Load the consolidated DataSources/mimo-tc/mimo-tc.json dataset and verify its checksum."""
    tc_dir = ROOT / "DataSources/mimo-tc"
    manifest = json.loads((tc_dir / "SOURCE.json").read_text())
    for name, checksum in manifest["files"].items():
        if hashlib.sha256((tc_dir / name).read_bytes()).hexdigest() != checksum:
            raise ValueError(f"mimo-tc data changed: {name}; put edits in Resources/Lexicon instead")
    return json.loads((tc_dir / "mimo-tc.json").read_text())


def load_mimo_sc_dataset() -> dict:
    """Load the consolidated DataSources/mimo-sc/mimo-sc.json dataset and verify its checksum."""
    sc_dir = ROOT / "DataSources/mimo-sc"
    manifest = json.loads((sc_dir / "SOURCE.json").read_text())
    for name, checksum in manifest["files"].items():
        if hashlib.sha256((sc_dir / name).read_bytes()).hexdigest() != checksum:
            raise ValueError(f"mimo-sc source checksum mismatch: {name}")
    return json.loads((sc_dir / "mimo-sc.json").read_text())


def fetch_and_build_mimo_datasets(converter: str, encode_fn) -> dict:
    """Download McBopomofo, OpenCC, jieba, and pinyin-pro to rebuild mimo-tc.json & mimo-sc.json."""
    tc_dir = ROOT / "DataSources/mimo-tc"
    sc_dir = ROOT / "DataSources/mimo-sc"

    bpmf_base = _download_text(f"{MCBOPOMOFO_BASE}/BPMFBase.txt")
    bpmf_mappings = _download_text(f"{MCBOPOMOFO_BASE}/BPMFMappings.txt")
    phrase_occ = _download_text(f"{MCBOPOMOFO_BASE}/phrase.occ")
    het1 = _download_text(f"{MCBOPOMOFO_BASE}/heterophony1.list")
    het2 = _download_text(f"{MCBOPOMOFO_BASE}/heterophony2.list")
    het3 = _download_text(f"{MCBOPOMOFO_BASE}/heterophony3.list")
    st_text = _download_text(f"{OPENCC_BASE}/data/dictionary/STPhrases.txt")
    tw_text = _download_text(f"{OPENCC_BASE}/data/dictionary/TWPhrases.txt")
    hk_text = _download_text(f"{OPENCC_BASE}/data/dictionary/HKPhrases.txt")
    opencc_license = _download_text(f"{OPENCC_BASE}/LICENSE")
    jieba_license = _download_text(f"{JIEBA_BASE}/LICENSE")
    jieba_big = _download_text(f"{JIEBA_BASE}/extra_dict/dict.txt.big")
    complete_raw = json.loads(_download_text(f"{PINYIN_PRO_BASE}/packages/data/json/complete.json"))

    (tc_dir / "OPENCC-LICENSE.txt").write_text(opencc_license)
    (tc_dir / "JIEBA-LICENSE.txt").write_text(jieba_license)

    freq = {f[0]: int(f[1]) for ln in phrase_occ.splitlines() if len(f := ln.split()) == 2}
    mapping, entries, skipped_tc = {}, {}, 0
    for line in bpmf_base.splitlines():
        fields = line.split()
        if len(fields) < 4:
            continue
        word, bpmf, py = fields[:3]
        py = py.lower().replace("ü", "v")
        if not py[-1:].isdigit():
            py += "1"
        if bpmf.startswith("ㄝ"):
            py = "eh" + py[-1]
        if encode_fn(py) is None:
            skipped_tc += 1
            continue
        mapping.setdefault(bpmf, py)
        if all(ord(c) >= 0x3400 for c in word):
            entries[(word, py)] = max(1, round(700 * math.log1p(freq.get(word, 0))))
    for line in bpmf_mappings.splitlines():
        fields = line.split()
        if len(fields) < 2:
            continue
        word, syls = fields[0], fields[1:]
        if len(syls) > 12 or len(word) != len(syls) or any(p not in mapping for p in syls):
            skipped_tc += 1
            continue
        entries[(word, " ".join(mapping[p] for p in syls))] = max(1, round(700 * math.log1p(freq.get(word, 0))))
    tiers = [collections.defaultdict(set) for _ in range(3)]
    for tier, text_src in zip(tiers, (het1, het2, het3)):
        for line in text_src.splitlines():
            if len(f := line.split()) == 2 and f[1] in mapping:
                tier[f[0]].add(mapping[f[1]])
    for (word, code) in list(entries):
        if len(word) == 1 and word in tiers[0] and code not in tiers[0][word]:
            w = entries[(word, code)]
            entries[(word, code)] = max(1, round(w * 0.72) if code in tiers[1][word] else round(w * 0.45) if code in tiers[2][word] else w // 8)
    words_sorted = sorted({w for w, _ in entries})
    conv = subprocess.run([converter], input="\n".join(words_sorted) + "\n", text=True, capture_output=True, check=True).stdout.splitlines()
    hans_map = dict(zip(words_sorted, conv))
    tc_entries = [[w, hans_map[w], c, wt] for (w, c), wt in sorted(entries.items(), key=lambda x: (x[0][0], hans_map[x[0][0]], x[0][1]))]

    jieba_freq = {p[0]: int(p[1]) for ln in jieba_big.splitlines() if len(p := ln.split()) >= 2 and _is_cjk_phrase(p[0], 2, 8) and p[1].isdigit()}
    st_map = {h: t.split()[0] for ln in st_text.splitlines() if "\t" in ln and not ln.startswith("#") for h, t in [ln.split("\t", 1)] if _is_cjk_phrase(h, 2, 8) and len(h) == len(t.split()[0]) and h != t.split()[0]}
    regional_tw_hk = {}
    for src_t in (tw_text, hk_text):
        for ln in src_t.splitlines():
            if "\t" in ln and not ln.startswith("#"):
                for hant in ln.split("\t", 1)[1].split():
                    if _is_cjk_phrase(hant, 2, 6):
                        regional_tw_hk[hant] = max(regional_tw_hk.get(hant, 0), log_prob_weight(jieba_freq.get(hant, 0), 320.0, SMOOTHING_FLOOR_I16))

    tc_payload = {"name": "mimo-tc", "version": "1.0", "license": "MIT AND Apache-2.0", "skipped_rows": skipped_tc, "entry_count": len(tc_entries), "st_phrase_count": len(st_map), "regional_phrase_count": len(regional_tw_hk), "entries": tc_entries, "st_map": st_map, "regional_tw_hk": regional_tw_hk}
    (tc_dir / "mimo-tc.json").write_text(json.dumps(tc_payload, ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n")

    valid_sc, skipped_sc = [], 0
    for hans, values in complete_raw.items():
        if not 1 <= len(hans) <= 12 or not all(0x3400 <= ord(c) <= 0x9FFF for c in hans):
            skipped_sc += 1; continue
        toned = [numbered_pinyin(p) for p in values[0].split()]
        if len(toned) != len(hans) or not all(encode_fn(x) for x in toned):
            skipped_sc += 1; continue
        r_str, f_val = " ".join(toned), float(values[1])
        valid_sc.append([hans, r_str, f_val])

    sc_payload = {"name": "mimo-sc", "version": "1.0", "license": "MIT", "excluded_invalid_rows": skipped_sc, "entry_count": len(valid_sc), "entries": valid_sc, "upstreams": ["zh-lx/pinyin-pro (MIT)"]}
    (sc_dir / "mimo-sc.json").write_text(json.dumps(sc_payload, ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n")
    return {"mimo_tc_entries": len(tc_entries), "mimo_sc_entries": len(valid_sc)}
