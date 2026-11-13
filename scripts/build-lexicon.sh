#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build
swiftc scripts/ConvertScript.swift -o .build/convert-script
python3 scripts/compile-native-lexicon.py "${1:-.build/lexicon}" "$PWD/.build/convert-script"
python3 scripts/copy-lexicon-notices.py "${1:-.build/lexicon}/Licenses"
