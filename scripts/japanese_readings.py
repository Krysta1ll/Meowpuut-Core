"""Generate explicit pinyin-kanji plus romaji-kana keys for reviewed Japanese words."""
import re

_ROWS = {
    "あいうえお": ("a", "i", "u", "e", "o"),
    "かきくけこ": ("ka", "ki", "ku", "ke", "ko"),
    "さしすせそ": ("sa", "shi", "su", "se", "so"),
    "たちつてと": ("ta", "chi", "tsu", "te", "to"),
    "なにぬねの": ("na", "ni", "nu", "ne", "no"),
    "はひふへほ": ("ha", "hi", "fu", "he", "ho"),
    "まみむめも": ("ma", "mi", "mu", "me", "mo"),
    "やゆよ": ("ya", "yu", "yo"),
    "らりるれろ": ("ra", "ri", "ru", "re", "ro"),
    "わをん": ("wa", "wo", "n"),
    "がぎぐげご": ("ga", "gi", "gu", "ge", "go"),
    "ざじずぜぞ": ("za", "ji", "zu", "ze", "zo"),
    "だぢづでど": ("da", "ji", "zu", "de", "do"),
    "ばびぶべぼ": ("ba", "bi", "bu", "be", "bo"),
    "ぱぴぷぺぽ": ("pa", "pi", "pu", "pe", "po"),
}
_KANA = {char: roman for chars, values in _ROWS.items() for char, roman in zip(chars, values)}
_COMPOUND = {
    **{base + small: prefix + ending for base, prefix in
       (("き", "ky"), ("ぎ", "gy"), ("し", "sh"), ("じ", "j"),
        ("ち", "ch"), ("に", "ny"), ("ひ", "hy"), ("び", "by"),
        ("ぴ", "py"), ("み", "my"), ("り", "ry"))
       for small, ending in (("ゃ", "a"), ("ゅ", "u"), ("ょ", "o"))},
}


def pinyin_kana(surface, chinese_syllables):
    """Use reviewed pinyin for each kanji and romanise written kana only."""
    syllables = iter(chinese_syllables)
    out = []
    index = 0
    while index < len(surface):
        char = surface[index]
        if "\u3400" <= char <= "\u9fff":
            try:
                syllable = next(syllables)
            except StopIteration as exc:
                raise ValueError(f"missing pinyin for {surface}") from exc
            if not re.fullmatch(r"[a-z]+", syllable):
                raise ValueError(f"invalid pinyin for {surface}: {syllable}")
            out.append(syllable)
            index += 1
            continue
        if "ァ" <= char <= "ヶ":
            char = chr(ord(char) - 0x60)
        if char == "っ":
            if index + 1 >= len(surface) or surface[index + 1] not in _KANA:
                raise ValueError(f"unsupported small tsu in {surface}")
            out.append(_KANA[surface[index + 1]][0])
            index += 1
            continue
        next_char = surface[index + 1] if index + 1 < len(surface) else ""
        if "ァ" <= next_char <= "ヶ":
            next_char = chr(ord(next_char) - 0x60)
        pair = char + next_char
        if pair in _COMPOUND:
            out.append(_COMPOUND[pair]); index += 2
        elif char in _KANA:
            out.append(_KANA[char]); index += 1
        else:
            raise ValueError(f"unsupported written kana in {surface}: {char}")
    if next(syllables, None) is not None:
        raise ValueError(f"extra pinyin for {surface}")
    return "".join(out)
