#!/usr/bin/env python3
"""Builds Resources/Lexicon/associations.json, the emoji and kaomoji offered for
a word, from four files:

  Resources/Lexicon/emoji.tsv                 hand-written: words, then up to three emoji
  Resources/Lexicon/kaomoji.tsv               hand-written: group, kaomoji, words
  DataSources/cldr-emoji/cldr-emoji.tsv       Chinese words from the Unicode CLDR annotations
  DataSources/cldr-emoji/cldr-emoji-ja.tsv    Japanese words from the same CLDR release

A word of the hand-written files is Japanese when it has kana or starts with
`ja:` (ja:手紙 is a letter); every other word is Chinese. The two languages are
kept apart, because the same characters can mean different things.

The result is {"zh": {word: [[emoji], [kaomoji]]}, "ja": {...}}: hand-written
emoji first, then CLDR's, three at most; then up to six kaomoji, the ones
listed for the word itself before the ones of its group.

  python3 scripts/build-emoji-associations.py
      check the hand-written files and rebuild associations.json
  python3 scripts/build-emoji-associations.py --cldr zh.xml zh_Hant.xml
      first rebuild cldr-emoji.tsv from CLDR's common/annotations files
  python3 scripts/build-emoji-associations.py --cldr-ja ja.xml
      first rebuild cldr-emoji-ja.tsv
  (put the sha256 the script prints in DataSources/cldr-emoji/SOURCE.json)
"""
import collections, hashlib, json, re, sys, unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EMOJI = ROOT / "Resources/Lexicon/emoji.tsv"
KAOMOJI = ROOT / "Resources/Lexicon/kaomoji.tsv"
TABLE = ROOT / "DataSources/cldr-emoji/cldr-emoji.tsv"
TABLE_JA = ROOT / "DataSources/cldr-emoji/cldr-emoji-ja.tsv"
OUT = ROOT / "Resources/Lexicon/associations.json"
# A keyword shared by more emoji than this says nothing (手, 臉, 動物).
MAX_EMOJI_PER_WORD = 6
CLDR_PER_WORD = 2
EMOJI_PER_WORD = 3
KAOMOJI_PER_WORD = 6
KAOMOJI_LENGTH = 24


def is_han(ch):
    return "㐀" <= ch <= "鿿"


def is_kana(ch):
    return "ぁ" <= ch <= "ヿ"


def is_word(text, japanese=False):
    if japanese:
        return 1 <= len(text) <= 6 and all(is_han(ch) or is_kana(ch) for ch in text)
    return 1 <= len(text) <= 4 and all(is_han(ch) for ch in text)


def emoji(cp):
    """The emoji as typed, or None for symbols that are not emoji (→, ©, ≠)."""
    points = [ord(ch) for ch in cp]
    if all(p >= 0x1F000 or p in (0x200D, 0xFE0F) for p in points):
        return cp
    if len(points) == 1 and (0x2600 <= points[0] <= 0x27BF or 0x231A <= points[0] <= 0x23FA or points[0] in (0x2B50, 0x2B55)):
        return cp + "️"  # emoji presentation, not the text glyph
    return None


def closeness(word, name):
    """0: the emoji's name is the word; 1: one contains the other; 2: they share
    a character (牛奶 and 一杯奶 🥛 before 黄油 🧈); 3: only a keyword."""
    if word == name:
        return 0
    if word in name or name in word:
        return 1
    return 2 if set(word) & set(name) else 3


def read_cldr(paths, japanese=False):
    # word -> [(file, closeness, order, emoji)]: zh before zh_Hant (你好 is 👋 in
    # zh, 🖖 in zh_Hant), then the emoji whose name is closest to the word.
    words = collections.defaultdict(list)
    order = 0
    for file, path in enumerate(paths):
        text = Path(path).read_text(encoding="utf-8")
        names = dict(re.findall(r'<annotation cp="([^"]+)" type="tts">([^<]+)</annotation>', text))
        for cp, keywords in re.findall(r'<annotation cp="([^"]+)">([^<]+)</annotation>', text):
            shown = emoji(cp)
            if not shown:
                continue
            order += 1
            name = names.get(cp, "").strip()
            for word in {k.strip() for k in keywords.split("|")} | {name}:
                # A single character only names an emoji (猫 🐈); as a keyword it is too loose (好 🫡).
                if is_word(word, japanese) and (len(word) > 1 or word == name):
                    words[word].append((file, closeness(word, name), order, shown))
    table = {}
    for word, found in words.items():
        unique = {}
        for item in sorted(found):
            unique.setdefault(item[3], item)
        if len(unique) <= MAX_EMOJI_PER_WORD:
            table[word] = [item[3] for item in sorted(unique.values())][:CLDR_PER_WORD]
    return table


def write_cldr(target, paths, japanese=False):
    table = read_cldr(paths, japanese)
    target.write_text("".join(f"{w}\t{' '.join(e)}\n" for w, e in sorted(table.items())), encoding="utf-8")
    for path in paths:
        print(Path(path).name, hashlib.sha256(Path(path).read_bytes()).hexdigest())
    print(target.name, hashlib.sha256(target.read_bytes()).hexdigest())


def language(word):
    """("ja", 手紙) for ja:手紙 and for a word with kana, ("zh", word) otherwise."""
    if word.startswith("ja:"):
        return "ja", word[3:]
    return ("ja" if any(is_kana(ch) for ch in word) else "zh"), word


def rows(path):
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if line and not line.startswith("#"):
            yield number, line.split("\t")


def read_emoji(problems):
    """{"zh": {word: [emoji]}, "ja": {...}} from emoji.tsv."""
    table = {"zh": {}, "ja": {}}
    for number, fields in rows(EMOJI):
        where = f"{EMOJI.name}:{number}"
        if len(fields) != 2:
            problems.append(f"{where}: expected words, a tab, then emoji")
            continue
        symbols = fields[1].split()
        if not 1 <= len(symbols) <= EMOJI_PER_WORD:
            problems.append(f"{where}: one to {EMOJI_PER_WORD} emoji")
        for symbol in symbols:
            if not any(ord(ch) >= 0x2190 for ch in symbol) or any(ch.isalnum() and ord(ch) < 0x2190 for ch in symbol):
                problems.append(f"{where}: {symbol} is not an emoji")
        for item in fields[0].split():
            lang, word = language(item)
            if word in table[lang]:
                problems.append(f"{where}: {item} is listed twice")
            table[lang][word] = symbols
    return table


def read_kaomoji(problems):
    """{"zh": {word: [kaomoji]}, "ja": {...}} from kaomoji.tsv: the kaomoji
    that list the word themselves, then the ones under a words row with it."""
    own = {"zh": collections.defaultdict(list), "ja": collections.defaultdict(list)}
    shared = {"zh": collections.defaultdict(list), "ja": collections.defaultdict(list)}
    seen, groups, words_row = set(), [], []
    for number, fields in rows(KAOMOJI):
        where = f"{KAOMOJI.name}:{number}"
        fields += [""] * (3 - len(fields))
        if len(fields) != 3 or not fields[0]:
            problems.append(f"{where}: expected group, kaomoji, words")
            continue
        group, text, words = fields[0].split("|")[0], fields[1], [language(w) for w in fields[2].split()]
        if not groups or groups[-1] != group:
            if group in groups:
                problems.append(f"{where}: the rows of {group} are not together")
            groups.append(group)
            words_row = []
        if not text:
            words_row = words
            continue
        if "|" in fields[0]:
            problems.append(f"{where}: only a words row gives the Simplified name")
        if text in seen:
            problems.append(f"{where}: {text} is listed twice")
        seen.add(text)
        if text != text.strip() or len(text) > KAOMOJI_LENGTH:
            problems.append(f"{where}: {text} has spaces around it or is longer than {KAOMOJI_LENGTH} characters")
        for ch in text:
            # No control or format characters, and no right-to-left letters:
            # they reorder the text typed after them.
            if unicodedata.category(ch) in ("Cc", "Cf", "Zl", "Zp", "Co", "Cn") or unicodedata.bidirectional(ch) in ("R", "AL"):
                problems.append(f"{where}: {text} has U+{ord(ch):04X}, which a text field may not show in place")
        for lang, word in words:
            own[lang][word].append(text)
        for lang, word in words_row:
            shared[lang][word].append(text)
    table = {}
    for lang in ("zh", "ja"):
        table[lang] = {}
        for word in list(own[lang]) + list(shared[lang]):
            both = own[lang].get(word, []) + shared[lang].get(word, [])
            table[lang][word] = list(dict.fromkeys(both))[:KAOMOJI_PER_WORD]
    return table, len(seen), len(groups)


def read_table(path):
    return {word: symbols.split(" ") for word, symbols in
            (line.split("\t") for line in path.read_text(encoding="utf-8").splitlines())}


def other_kana(word):
    """ねこ for ネコ and ネコ for ねこ; None for a word that is not all hiragana or all katakana."""
    if all("ぁ" <= ch <= "ゖ" for ch in word):
        return "".join(chr(ord(ch) + 0x60) for ch in word)
    if all("ァ" <= ch <= "ヶ" for ch in word):
        return "".join(chr(ord(ch) - 0x60) for ch in word)
    return None


def main():
    args = sys.argv[1:]
    if args[:1] == ["--cldr"]:
        write_cldr(TABLE, args[1:])
    if args[:1] == ["--cldr-ja"]:
        write_cldr(TABLE_JA, args[1:], japanese=True)
    problems = []
    hand = read_emoji(problems)
    kaomoji, kaomoji_count, group_count = read_kaomoji(problems)
    if problems:
        sys.exit("\n".join(problems))
    result = {}
    for lang, cldr in (("zh", read_table(TABLE)), ("ja", read_table(TABLE_JA))):
        entries = {}
        for word in list(hand[lang]) + list(kaomoji[lang]) + list(cldr):
            if word in entries:
                continue
            # ☕ and ☕️ are one emoji, with and without the presentation selector.
            symbols, seen = [], set()
            for symbol in hand[lang].get(word, []) + cldr.get(word, []):
                if symbol.replace("\uFE0F", "") not in seen:
                    seen.add(symbol.replace("\uFE0F", ""))
                    symbols.append(symbol)
            entries[word] = [symbols[:EMOJI_PER_WORD if word in hand[lang] else CLDR_PER_WORD], kaomoji[lang].get(word, [])]
        if lang == "ja":
            # A word in one kana also answers for the other (ねこ, ネコ).
            for word, entry in list(entries.items()):
                other = other_kana(word)
                if other and other not in entries:
                    entries[other] = entry
        result[lang] = entries
    # One word a line, so a change shows as that line.
    lines = []
    for lang in ("zh", "ja"):
        body = ",\n".join(json.dumps(word, ensure_ascii=False) + ":" +
                          json.dumps(entry if entry[1] else entry[:1], ensure_ascii=False, separators=(",", ":"))
                          for word, entry in sorted(result[lang].items()))
        lines.append(json.dumps(lang) + ":{\n" + body + "\n}")
    OUT.write_text("{\n" + ",\n".join(lines) + "\n}\n", encoding="utf-8")
    json.loads(OUT.read_text(encoding="utf-8"))
    print(f"{OUT.relative_to(ROOT)}: {len(result['zh'])} Chinese and {len(result['ja'])} Japanese words; "
          f"hand-written: {len(hand['zh'])} + {len(hand['ja'])} words with emoji, {kaomoji_count} kaomoji in {group_count} groups")


if __name__ == "__main__":
    main()
