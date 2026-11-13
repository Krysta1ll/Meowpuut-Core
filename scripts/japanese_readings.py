"""Japanese readings: romaji to kana, kana to Hepburn, the lexicon key, and pinyin aliases.

to_kana and normalise_key follow crates/meowpuut-core/src/romaji.rs. pinyin_kana is the alias rule
of the reviewed words and raises on input it cannot spell; the other functions return None instead.

    python3 scripts/japanese_readings.py --check [--snapshot mimo-jp.tsv]

checks this file against the data files in crates/meowpuut-core/tests/data.
"""
import re
import sys
from pathlib import Path

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


# Romaji table of romaji.rs (the Swift table plus dhu and thu). Hepburn tokens are strict.
_STRICT_TOKENS = """
a:あ ba:ば be:べ bi:び bo:ぼ bu:ぶ bya:びゃ bye:びぇ byo:びょ byu:びゅ cha:ちゃ che:ちぇ chi:ち cho:ちょ chu:ちゅ
da:だ de:で dhi:でぃ dhu:でゅ di:ぢ do:ど du:づ dwu:どぅ e:え fa:ふぁ fe:ふぇ fi:ふぃ fo:ふぉ fu:ふ fyu:ふゅ ga:が
ge:げ gi:ぎ go:ご gu:ぐ gya:ぎゃ gye:ぎぇ gyo:ぎょ gyu:ぎゅ ha:は he:へ hi:ひ ho:ほ hya:ひゃ hye:ひぇ hyo:ひょ hyu:ひゅ
i:い ja:じゃ je:じぇ ji:じ jo:じょ ju:じゅ ka:か ke:け ki:き ko:こ ku:く kya:きゃ kye:きぇ kyo:きょ kyu:きゅ ma:ま
me:め mi:み mo:も mu:む mya:みゃ mye:みぇ myo:みょ myu:みゅ na:な ne:ね ni:に no:の nu:ぬ nya:にゃ nye:にぇ nyo:にょ
nyu:にゅ o:お pa:ぱ pe:ぺ pi:ぴ po:ぽ pu:ぷ pya:ぴゃ pye:ぴぇ pyo:ぴょ pyu:ぴゅ ra:ら re:れ ri:り ro:ろ ru:る rya:りゃ
rye:りぇ ryo:りょ ryu:りゅ sa:さ se:せ sha:しゃ she:しぇ shi:し sho:しょ shu:しゅ so:そ su:す ta:た te:て thi:てぃ
thu:てゅ to:と tsu:つ twu:とぅ u:う va:ゔぁ ve:ゔぇ vi:ゔぃ vo:ゔぉ vu:ゔ wa:わ we:うぇ who:うぉ wi:うぃ wo:を ya:や
ye:いぇ yo:よ yu:ゆ za:ざ ze:ぜ zo:ぞ zu:ず
"""
# Kunrei spellings and x/l small kana: converted, but not strict.
_LOOSE_TOKENS = """
cya:ちゃ cyo:ちょ cyu:ちゅ hu:ふ jya:じゃ jyo:じょ jyu:じゅ la:ぁ le:ぇ li:ぃ lo:ぉ ltsu:っ ltu:っ lu:ぅ lwa:ゎ
lya:ゃ lyo:ょ lyu:ゅ si:し sya:しゃ syo:しょ syu:しゅ ti:ち tu:つ tya:ちゃ tyo:ちょ tyu:ちゅ xa:ぁ xe:ぇ xi:ぃ xo:ぉ
xtsu:っ xtu:っ xu:ぅ xwa:ゎ xya:ゃ xyo:ょ xyu:ゅ zi:じ zya:じゃ zyo:じょ zyu:じゅ
"""
TOKENS = {}
for _spec, _strict in ((_STRICT_TOKENS, True), (_LOOSE_TOKENS, False)):
    for _item in _spec.split():
        _token, _kana = _item.split(":")
        TOKENS[_token] = (_kana, _strict)

# Kana to Hepburn token: the one strict token for each kana.
_HEPBURN = {}
for _token, (_kana, _strict) in sorted(TOKENS.items()):
    if _strict:
        assert _kana not in _HEPBURN, f"two Hepburn tokens for {_kana}"
        _HEPBURN[_kana] = _token
# In an alias the kana is spelled as people type it: ぢ and づ as ji and zu, as pinyin_kana does.
_ALIAS_SPELLING = {**_HEPBURN, "ぢ": "ji", "づ": "zu"}

_VOWEL_OF = {}
for _row, _vowel in (("あかさたなはまやらわがざだばぱぁゃゎ", "あ"),
                     ("いきしちにひみりぎじぢびぴぃ", "い"),
                     ("うくすつぬふむゆるぐずづぶぷぅゅゔっ", "う"),
                     ("えけせてねへめれげぜでべぺぇ", "え"),
                     ("おこそとのほもよろをごぞどぼぽぉょ", "お")):
    for _ch in _row:
        _VOWEL_OF[_ch] = _vowel


def _lower(ch):
    return ch.lower() if "A" <= ch <= "Z" else ch


def to_kana(text):
    """Romaji to kana as romaji.rs to_kana does. Returns (kana, complete, strict).

    complete: no ASCII letter is left. strict: only Hepburn tokens were used and nothing was
    copied unconverted. Never raises on a str.
    """
    kana, complete, strict, i = [], True, True, 0
    while i < len(text):
        raw = text[i]
        nxt = _lower(text[i + 1]) if i + 1 < len(text) else None
        if not raw.isascii():
            # Copied as it is, with the doubling rule of the Swift code.
            if text[i + 1:i + 2] == raw:
                kana.append("っ")
            else:
                kana.append(raw)
                strict = False
            i += 1
            continue
        key = _lower(raw)
        if key == "-":
            kana.append("ー"); i += 1; continue
        if key == "'":
            i += 1; continue
        if key == "n":
            if nxt == "'":
                step = 2
            elif nxt is None:
                step = 1
            elif nxt == "n":
                step = 2 if i + 2 == len(text) else 1  # a final "nn" is one ん
            elif nxt not in "aiueoy":
                step = 1
            else:
                step = 0
            if step:
                kana.append("ん"); i += step; continue
        if nxt == key and key not in "aiueon":
            kana.append("っ"); i += 1; continue
        for count in range(min(4, len(text) - i), 0, -1):
            hit = TOKENS.get("".join(_lower(ch) for ch in text[i:i + count]))
            if hit:
                kana.append(hit[0]); strict = strict and hit[1]; i += count
                break
        else:
            kana.append(key)
            complete = complete and not ("a" <= key <= "z")
            strict = False
            i += 1
    return "".join(kana), complete, strict


def romaji_to_kana(text):
    """Kana for romaji, or None when a letter is left over (`kak`) or text is not a str."""
    if not isinstance(text, str):
        return None
    kana, complete, _ = to_kana(text)
    return kana if complete else None


def to_hiragana(text):
    """Katakana U+30A1..U+30F6 to hiragana; ー and everything else unchanged."""
    return "".join(chr(ord(ch) - 0x60) if "\u30a1" <= ch <= "\u30f6" else ch for ch in text)


def normalise_key(kana):
    """Lexicon key (design 7.2): katakana to hiragana, ー to the vowel kana before it.

    ー stays ー when nothing with a vowel comes before it. None when kana is not a str.
    """
    if not isinstance(kana, str):
        return None
    out = []
    for ch in to_hiragana(kana):
        if ch == "ー" and out:
            ch = _VOWEL_OF.get(out[-1], ch)
        out.append(ch)
    return "".join(out)


def _token_at(kana, i, table):
    """(spelling, length) of the kana token at i, two kana before one; None when there is none."""
    if kana[i:i + 2] in table and len(kana[i:i + 2]) == 2:
        return table[kana[i:i + 2]], 2
    if kana[i:i + 1] in table:
        return table[kana[i]], 1
    return None


def _spell(kana, table, apostrophe):
    """Letters for a hiragana run, or None. っ doubles the next consonant, ー repeats the vowel."""
    out, i = [], 0
    while i < len(kana):
        ch = kana[i]
        if ch == "ー":
            if not out or out[-1][-1] not in "aiueo":
                return None
            out.append(out[-1][-1]); i += 1
        elif ch == "っ":
            head = _token_at(kana, i + 1, table)
            if not head or head[0][0] in "aiueon":
                return None
            out.append(head[0][0]); i += 1
        elif ch == "ん":
            head = _token_at(kana, i + 1, table)
            before_vowel = kana[i + 1:i + 2] == "ん" or (head and head[0][0] in "aiueoy")
            out.append("n'" if apostrophe and before_vowel else "n"); i += 1
        else:
            token = _token_at(kana, i, table)
            if not token:
                return None
            out.append(token[0]); i += token[1]
    return "".join(out)


def kana_to_hepburn(kana):
    """Hepburn spelling that to_kana turns back into kana (same key), or None.

    ん is n, or n' before a vowel, y or ん. っ doubles the next consonant; ー doubles the vowel.
    None when the reading needs an x kana (word-final っ, a small kana on its own, ゕ, ゖ).
    """
    if not isinstance(kana, str) or not kana:
        return None
    romaji = _spell(to_hiragana(kana), _HEPBURN, apostrophe=True)
    if romaji is None:
        return None
    back, complete, strict = to_kana(romaji)
    return romaji if complete and strict and normalise_key(back) == normalise_key(kana) else None


def _is_kanji(ch):
    code = ord(ch)
    return (0x3400 <= code <= 0x9FFF or 0xF900 <= code <= 0xFAFF
            or 0x20000 <= code <= 0x323AF)


def load_kanji_table(path):
    """Resources/Lexicon/japanese-kanji-pinyin.tsv as {kanji: [reading, second reading]}.

    Lines: kanji<TAB>reading [second reading]<TAB>note; # starts a comment. A line that does not
    fit raises ValueError: the table is written by hand and a typo must stop the build.
    """
    table = {}
    for number, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split("\t")
        readings = fields[1].split() if len(fields) >= 2 else []
        if (len(fields[0]) != 1 or not _is_kanji(fields[0]) or not 1 <= len(readings) <= 2
                or not all(re.fullmatch(r"[a-z]+", reading) for reading in readings)
                or fields[0] in table):
            raise ValueError(f"{path}:{number}: expected kanji<TAB>reading [second reading]<TAB>note")
        table[fields[0]] = readings
    return table


def alias_keys(surface, pinyin, rule="char", table=None):
    """Pinyin alias keys of a word: pinyin for each kanji, letters for the kana. Returns a list
    of one or two keys, or None when the word gets no alias.

    surface: the written word. pinyin: snapshot column 4, one syllable per kanji and per 々,
    as a str or a list. rule: snapshot column 8; with `word` the syllables are used as they are,
    otherwise the table (load_kanji_table) replaces the reading of each kanji it lists and a
    second reading gives a second key, when only one kanji of the word has one. 々 repeats the
    kanji before it. None for: no kanji; a syllable count that does not match; a guessed
    syllable (`yu*`) with no table line; っ before a kanji or at the end; a small kana, ー or ヶ
    that cannot be spelled. The gate of design section 8 is not applied here.
    """
    if not isinstance(surface, str) or not surface:
        return None
    if isinstance(pinyin, str):
        syllables = pinyin.split()
    elif isinstance(pinyin, (list, tuple)) and all(isinstance(item, str) for item in pinyin):
        syllables = list(pinyin)
    elif pinyin is None:
        syllables = []
    else:
        return None
    table = table or {}
    per_char = rule != "word"
    choices = []  # per kanji: list of readings; per kana run: [letters]
    kanji_seen, position, i = 0, 0, 0
    while i < len(surface):
        ch = surface[i]
        if _is_kanji(ch) or ch == "々":
            if position >= len(syllables):
                return None
            syllable = syllables[position]
            position += 1
            kanji_seen += 1
            if ch == "々" and per_char:
                if not choices or choices[-1][0] != "kanji":
                    return None
                choices.append(("repeat", None))
            elif per_char and ch in table:
                choices.append(("kanji", table[ch]))
            elif re.fullmatch(r"[a-z]+", syllable):
                choices.append(("kanji", [syllable]))
            else:
                return None  # a guess (`*`) or not a syllable
            i += 1
            continue
        start = i
        while i < len(surface) and not (_is_kanji(surface[i]) or surface[i] == "々"):
            i += 1
        run = to_hiragana(surface[start:i])
        if "ゕ" in run or "ゖ" in run:
            return None  # ヵ ヶ are read か or が, and Chinese readers take ヶ for 个
        letters = _spell(run, _ALIAS_SPELLING, apostrophe=False)
        if letters is None:
            return None
        choices.append(("kana", [letters]))
    if not kanji_seen or position != len(syllables):
        return None
    two = [index for index, (kind, readings) in enumerate(choices)
           if kind == "kanji" and len(readings) == 2]
    variants = [0] if len(two) != 1 else [0, 1]
    keys = []
    for variant in variants:
        out, last = [], None
        for index, (kind, readings) in enumerate(choices):
            if kind == "repeat":
                out.append(last)
                continue
            reading = readings[variant] if index in two else readings[0]
            if kind == "kanji":
                last = reading
            out.append(reading)
        key = "".join(out)
        if key not in keys:
            keys.append(key)
    return keys


# --------------------------------------------------------------------------- check

def _check(snapshot):
    root = Path(__file__).resolve().parent.parent
    data = root / "crates/meowpuut-core/tests/data"
    failures = []

    def expect(label, got, want):
        if got != want:
            failures.append(f"{label}: got {got!r}, want {want!r}")

    rust = (root / "crates/meowpuut-core/src/romaji.rs").read_text(encoding="utf-8")
    rust_table = {token: (kana, strict == "true") for token, kana, strict in
                  re.findall(r'\("([a-z]+)", "([^"]+)", (true|false)\)', rust)}
    expect("token table equals romaji.rs", TOKENS, rust_table)

    rows = (data / "romaji_golden.tsv").read_text(encoding="utf-8").splitlines()
    for line in rows:
        text, want = line.split("\t", 1)
        kana, complete, strict = to_kana(text)
        expect(f"to_kana {text!r}", kana, want)
        expect(f"complete {text!r}", complete, not re.search("[A-Za-z]", kana))
        if strict and not complete:
            failures.append(f"strict but not complete: {text!r}")
    strict_rows = (data / "romaji_strict_cases.tsv").read_text(encoding="utf-8").splitlines()
    for line in strict_rows:
        text, want = line.split("\t", 1)
        expect(f"strict {text!r}", to_kana(text)[2], want == "1")
    key_rows = (data / "normalise_key_cases.tsv").read_text(encoding="utf-8").splitlines()
    for line in key_rows:
        kana, want = line.split("\t", 1)
        expect(f"normalise_key {kana!r}", normalise_key(kana), want)
        expect(f"normalise_key of a key {want!r}", normalise_key(want), want)

    fixed = [
        (to_kana("dhu"), ("でゅ", True, True)), (to_kana("thu"), ("てゅ", True, True)),
        (to_kana(""), ("", True, True)), (to_kana("KaWaIi")[0], "かわいい"),
        (to_kana("猫ka"), ("猫か", True, False)), (to_kana("kak"), ("かk", False, False)),
        (to_kana("a1"), ("あ1", True, False)), (to_kana("syasin"), ("しゃしん", True, False)),
        (to_kana("ko-hi-"), ("こーひー", True, True)),
        (romaji_to_kana("kak"), None), (romaji_to_kana("sin'ya"), "しんや"), (romaji_to_kana(None), None),
        (normalise_key("コーヒー"), "こおひい"), (normalise_key("ーあ"), "ーあ"), (normalise_key(3), None),
        (kana_to_hepburn("たべる"), "taberu"), (kana_to_hepburn("こーひー"), "koohii"),
        (kana_to_hepburn("コーヒー"), "koohii"), (kana_to_hepburn("れんあい"), "ren'ai"),
        (kana_to_hepburn("しんや"), "shin'ya"), (kana_to_hepburn("まっちゃ"), "maccha"),
        (kana_to_hepburn("きっぷ"), "kippu"), (kana_to_hepburn("ゔぁいおりん"), "vaiorin"),
        (kana_to_hepburn("ぢづ"), "didu"), (kana_to_hepburn("うんん"), "un'n"),
        (kana_to_hepburn("あっ"), None), (kana_to_hepburn("あぁ"), None), (kana_to_hepburn("っか"), "kka"),
        (kana_to_hepburn("んー"), None), (kana_to_hepburn("ゖ"), None), (kana_to_hepburn(""), None),
        (kana_to_hepburn("猫"), None), (kana_to_hepburn(None), None),
    ]
    for number, (got, want) in enumerate(fixed):
        expect(f"fixed case {number}", got, want)

    kanji_table = load_kanji_table(root / "Resources/Lexicon/japanese-kanji-pinyin.tsv") \
        if (root / "Resources/Lexicon/japanese-kanji-pinyin.tsv").exists() else {}
    small = {"着": ["zhuo"], "給": ["ji", "gei"], "行": ["xing", "hang"], "重": ["zhong", "chong"],
             "働": ["dong"], "楽": ["le", "yue"]}
    aliases = [
        (("食べる", "shi"), ["shiberu"]),
        (("推し活", "tui huo"), ["tuishihuo"]),
        (("散歩", "san bu", "char"), ["sanbu"]),
        (("着る", "zhe", "char", small), ["zhuoru"]),
        (("給料", "ji liao", "char", small), ["jiliao", "geiliao"]),
        (("行く", "xing", "char", small), ["xingku", "hangku"]),      # the gate keeps xingku only
        (("行", "hang", "word", small), ["hang"]),
        (("銀行", "yin hang", "word", small), ["yinhang"]),
        (("行楽", "xing le", "char", small), ["xingle"]),             # two kanji with two readings
        (("重々", "zhong zhong", "char", small), ["zhongzhong", "chongchong"]),
        (("人々", "ren ren"), ["renren"]),
        (("々", "ren"), None),
        (("働く", "dong* ku", "char", {}), None),
        (("働く", "dong*", "char", small), ["dongku"]),
        (("込む", "yu*"), None),
        (("ラーメン屋", "wu"), ["raamenwu"]),
        (("コーヒー豆", "dou"), ["koohiidou"]),
        (("板チョコ", "ban"), ["banchoko"]),
        (("赤ちゃん", "chi"), ["chichan"]),
        (("ウェブ上", "shang"), ["webushang"]),
        (("ヴァイオリン奏者", "zou zhe"), ["vaiorinzouzhe"]),
        (("持って", "chi"), ["chitte"]),
        (("マッチ箱", "xiang"), ["macchixiang"]),
        (("甥っ子", "sheng zi"), None),                              # っ before a kanji
        (("何かっ", "he"), None),                                   # word-final っ
        (("一ヶ月", "yi yue"), None),
        (("鼻ぁ", "bi"), None),                                     # small kana on its own
        (("縮み", "suo"), ["suomi"]),
        (("近づく", "jin"), ["jinzuku"]),
        (("𠮟る", "chi"), ["chiru"]),
        (("食べる", "shi", "char", small), ["shiberu"]),
        (("食べる", ""), None), (("食べる", "shi xx"), None), (("たべる", ""), None),
        (("音楽", "yin yue", "word", small), ["yinyue"]),
        (("楽しい", "le", "char", small), ["leshii", "yueshii"]),
        ((None, "a"), None), (("", ""), None), (("食べる", 3), None), (("食べる", [None]), None),
        (("食べる", ("shi",)), ["shiberu"]), (("食べる", "shi", None, None), ["shiberu"]),
    ]
    for args, want in aliases:
        expect(f"alias_keys{args[:3]}", alias_keys(*args), want)
    if kanji_table:
        expect("project table: 給料", alias_keys("給料", "ji liao", "char", kanji_table), ["jiliao", "geiliao"])
        expect("project table: 都合", alias_keys("都合", "dou he", "char", kanji_table)[0], "duhe")

    # The reviewed rows: no table, so the key equals what pinyin_kana gives today.
    reviewed = root / "Resources/Lexicon/japanese-reviewed.tsv"
    if not reviewed.exists():  # before step C2 the rows were in DataSources/mimo-jp
        reviewed = root / "DataSources/mimo-jp/mimo-jp.tsv"
    count = 0
    for line in reviewed.read_text(encoding="utf-8").splitlines():
        fields = line.split("|")
        if line.startswith("#") or len(fields) != 5 or not any(_is_kanji(ch) for ch in fields[1]):
            continue
        expect(f"reviewed {fields[1]}", alias_keys(fields[1], fields[3]), [pinyin_kana(fields[1], fields[3].split())])
        count += 1
    expect("reviewed rows with kanji found", count > 0, True)

    checked = 0
    for path in [data / "mimo_jp_snapshot_sample.tsv"] + ([Path(snapshot)] if snapshot else []):
        for line in path.read_text(encoding="utf-8").splitlines():
            fields = line.split("|")
            if line.startswith("#") or len(fields) != 8:
                continue
            kana, surface, romaji, pinyin, rule = fields[0], fields[1], fields[2], fields[3], fields[7]
            if romaji:
                back, complete, strict = to_kana(romaji)
                if not (complete and strict and normalise_key(back) == normalise_key(kana)):
                    failures.append(f"{path.name}: {line}: romaji gives {back}")
            if path.name != "mimo_jp_snapshot_sample.tsv":
                expect(f"kana_to_hepburn {kana}", kana_to_hepburn(kana) or "", romaji)
                try:
                    old = pinyin_kana(surface, pinyin.split())
                except ValueError:
                    old = None
                if old is not None and any("\u3400" <= ch <= "\u9fff" for ch in surface):
                    expect(f"alias {surface}", alias_keys(surface, pinyin, rule), [old])
            checked += 1

    for failure in failures[:20]:
        print(failure)
    print(f"{len(rows)} golden, {len(strict_rows)} strict, {len(key_rows)} key, {len(fixed)} fixed, "
          f"{len(aliases)} alias cases, {count} reviewed rows, {checked} snapshot rows: "
          f"{'FAIL ' + str(len(failures)) if failures else 'PASS'}")
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--check":
        snapshot = sys.argv[3] if len(sys.argv) >= 4 and sys.argv[2] == "--snapshot" else None
        sys.exit(_check(snapshot))
    print(__doc__.strip())
    sys.exit(2)
