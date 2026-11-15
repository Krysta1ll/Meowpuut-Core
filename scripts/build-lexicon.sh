#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build
converter="$(mktemp -d "$PWD/.build/converter.XXXXXX")"
trap 'rm -rf "$converter"' EXIT
swiftc scripts/ConvertScript.swift -o "$converter/convert-script"
python3 scripts/compile-native-lexicon.py "${1:-.build/lexicon}" "$converter/convert-script"
python3 -I scripts/mimo_en_binary.py "${1:-.build/lexicon}/mimo_en.bin"
python3 scripts/copy-lexicon-notices.py "${1:-.build/lexicon}/Licenses"
# The never-volunteer list as the clients read it (hashes, no readable text; Sources/NeverVolunteer.swift).
python3 scripts/blocked_terms.py --client "${1:-.build/lexicon}/never-volunteer.txt"
