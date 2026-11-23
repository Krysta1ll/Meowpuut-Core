#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build
converter="$(mktemp -d "$PWD/.build/converter.XXXXXX")"
trap 'rm -rf "$converter"' EXIT
swiftc scripts/ConvertScript.swift -o "$converter/convert-script"
python3 scripts/compile-native-lexicon.py "${1:-.build/lexicon}" "$converter/convert-script"
python3 -I scripts/mimo_en_binary.py "${1:-.build/lexicon}/mimo_en.bin"
# What may follow a committed Japanese word (DataSources/mimo-jp-next; crates/meowpuut-core/src/japanese_next.rs).
python3 -I scripts/mimo_jp_next.py pack "${1:-.build/lexicon}/mimo_jp_next.bin"
python3 scripts/copy-lexicon-notices.py "${1:-.build/lexicon}/Licenses"
# The never-volunteer list as the clients read it (hashes, no readable text; Sources/NeverVolunteer.swift).
python3 scripts/blocked_terms.py --client "${1:-.build/lexicon}/never-volunteer.txt"
if [ -f DataSources/mimo-trigrams/trigram_bridge.tsv.xz ] && [ -f DataSources/mimo-trigrams/skip1_bigram.tsv.xz ]; then
  python3 scripts/mimo_trigrams.py pack \
    --lexicon-dir "${1:-.build/lexicon}" \
    --trigrams DataSources/mimo-trigrams/trigram_bridge.tsv.xz \
    --skip1 DataSources/mimo-trigrams/skip1_bigram.tsv.xz \
    --tri-weight 0.00 --skip-weight 0.25 \
    --out "${1:-.build/lexicon}/mimo_trigrams.bin"
fi
if [ -f scripts/mimo_assoc.py ] && [ -d DataSources/mimo-everyday ]; then
  python3 scripts/mimo_assoc.py build \
    --lexicon-dir "${1:-.build/lexicon}" \
    --corpus-dirs DataSources/mimo-everyday \
    --out "${1:-.build/lexicon}/mimo_assoc.bin"
fi
