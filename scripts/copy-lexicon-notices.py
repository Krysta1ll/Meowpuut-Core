#!/usr/bin/env python3
"""Copy the notices needed to redistribute the generated lexicon."""
import argparse
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOTICES = {
    'Meowpuut-MIT.txt': 'LICENSE',
    'mimo-tc-MIT.txt': 'DataSources/mimo-tc/LICENSE.txt',
    'OpenCC-Apache-2.0.txt': 'DataSources/mimo-tc/OPENCC-LICENSE.txt',
    'jieba-MIT.txt': 'DataSources/mimo-tc/JIEBA-LICENSE.txt',
    'mimo-tc-SOURCE.json': 'DataSources/mimo-tc/SOURCE.json',
    'mimo-sc-MIT.txt': 'DataSources/mimo-sc/LICENSE',
    'mimo-sc-SOURCE.json': 'DataSources/mimo-sc/SOURCE.json',
    'CLDR-Unicode-3.0.txt': 'DataSources/cldr-emoji/LICENSE.txt',
    'cldr-emoji-SOURCE.json': 'DataSources/cldr-emoji/SOURCE.json',
    'mimo-jp-derived-BSD.txt': 'DataSources/mimo-jp/LICENSE.txt',
    'mimo-jp-BSD.txt': 'DataSources/mimo-jp/BSD.txt',
    'mimo-jp-COPYING.txt': 'DataSources/mimo-jp/COPYING.txt',
    'mimo-jp-SOURCE.json': 'DataSources/mimo-jp/SOURCE.json',
    'mimo-jp-Mozc-BSD.txt': 'DataSources/mimo-jp/MOZC-LICENSE.txt',
    'mimo-jp-Unicode-3.0.txt': 'DataSources/mimo-jp/UNICODE-LICENSE.txt',
    'mimo-pairs-SOURCE.json': 'DataSources/mimo-pairs/SOURCE.json',
    'mimo-pairs-Apache-2.0.txt': 'DataSources/mimo-pairs/Apache-2.0.txt',
    'mimo-pairs-CC-BY-4.0.txt': 'DataSources/mimo-pairs/CC-BY-4.0.txt',
    'mimo-names-SOURCE.json': 'DataSources/mimo-names/SOURCE.json',
    'mimo-phrases-SOURCE.json': 'DataSources/mimo-phrases/SOURCE.json',
    'mimo-pairs-Tatoeba-CC-BY-2.0-FR.html': 'DataSources/mimo-pairs/CC-BY-2.0-FR.html',
    'mimo-pairs-CommonVoice-CC0-1.0.txt': 'DataSources/mimo-pairs/CC0-1.0.txt',
    'mimo-latin-SOURCE.json': 'DataSources/mimo-latin/SOURCE.json',
}

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    destination = parser.parse_args().destination
    destination.mkdir(parents=True, exist_ok=True)
    for name, source in NOTICES.items():
        shutil.copyfile(ROOT / source, destination / name)
