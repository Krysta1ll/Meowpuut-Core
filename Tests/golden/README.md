# Goldens for Chinese typing

These files show that Chinese candidates did not change. They were first written from the tree
before the mimo-jp 2.0 work (sandbox base 44dd8bd) and rewritten on 2026-11-11 after the merge
with the lexicon weight rules ZY1, PL1, PL2, PL3 (they change Chinese candidates on purpose) and
the iOS keyboard changes (emoji in the association table, no typed letters in zhuyin, a reviewed
kana word such as すし listed as a word). Without ZY1–PL3 the core goldens matched row for row.
mimo_base.bin of the rewrite: 89d1063da28b67938255bd2d2d3e509742d9ef507f0e86e1ee6ff1737be13eb9.
Rewritten again on 2026-11-11 for 0.6.0: word-pair table, spoken counts and the everyday sentences, English words inside pinyin, mixed full syllables and initials, whole-input words and guesses ranked by score, the new row order after the first candidate, and a reviewed Japanese word before a Chinese reading made with English pieces. mimo_base.bin: 0ee5c72326b010afb8596f45ebf43ef80ae25500e4061ba5ab5d1074d349c4f9.
Rewritten on 2026-11-12 for emoji and kaomoji (client-candidates.tsv only; the core goldens did not change): the symbols of the first candidate (source -1, not flagged Japanese) come from emoji.tsv, kaomoji.tsv and CLDR, up to three emoji and six kaomoji, and are listed after every word. 933 of the 3,974 rows changed; in each only these symbols differ. No Chinese candidate moved; in 63 pinyin rows the Japanese words of the large list now come before the symbols. Same mimo_base.bin as above.
Rewritten on 2026-11-12 for 0.7.0: the lexicon counts now come from the project's own sentences and other neutral text, with phrases, names and working words added, and count evidence in the zhuyin weights. mimo_base.bin: 5bb5d1df1c352672ce8d1cdc9088e67e758dd2c934ff10c812de56cb978ebbfd.

| file | what |
| --- | --- |
| `chinese-pinyin-inputs.txt`, `chinese-zhuyin-inputs.txt` | inputs of `scripts/check-chinese-golden.py` |
| `chinese-pinyin-top5.tsv`, `chinese-zhuyin-top5.tsv` | core: candidate count and first five candidates after every key |
| `client-inputs.txt` | inputs of the client harness in `Sources/SmokeMixed.swift` |
| `client-candidates.tsv` | macOS backend: the whole candidate list per input, with source and Japanese flag |

## Check

```
bash scripts/build.sh
python3 scripts/check-chinese-golden.py
MEOWPUUT_GOLDEN_CHECK=1 .build/Meowpuut.app/Contents/MacOS/Meowpuut --smoke
```

`MEOWPUUT_GOLDEN_CHECK=strict` asks for identical lists. `MEOWPUUT_GOLDEN_CHECK=1` allows an added
Japanese candidate as long as no Chinese candidate moves (the rules are in the comment above
`runClientGolden`). A plain `--smoke` run does not look at the goldens.

## Rewrite

Only after a change to the Chinese lexicon or the Chinese candidates that was meant:

```
python3 scripts/check-chinese-golden.py --write
MEOWPUUT_GOLDEN_WRITE=1 .build/Meowpuut.app/Contents/MacOS/Meowpuut --smoke
```

Both golden headers hold the sha256 of `mimo_base.bin`, so a lexicon change shows as such.

## Lexicon hashes at the base (clean copy, `scripts/compile-native-lexicon.py`)

```
a493be4776d47c3445d9e5ec931c556072d0ab456cd678101411d568bcfac0ef  mimo_base.bin
8d287b719c3d666e2b09daae8a417d9bd40bfd0f468337c1b7f3ebbac4bbb8c5  native-pinyin.tsv
7eecce5bbf4536832c9dffe60805b2e43000fd4ffef1d184c773be78737abe30  native-zhuyin.tsv
f398650e6207b18127a4a38bd1384e896e2f1518f8d00077e25e9e226e2cfa6b  native-japanese.json
5970323a98dccc1bbae87ee99d285b23af4d89648905740284de20ac52a29b61  native-japanese.tsv
```
