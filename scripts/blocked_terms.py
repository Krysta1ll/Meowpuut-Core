"""Reader of Resources/Lexicon/blocked-terms.txt, the one list of contested or offensive terms.

The file has sections ([zh], [zh-keep], [zh-drop], [ja], [en], [emoji-key]; see its header). Every
script that counts text, builds the lexicon, the English or Japanese images or the emoji associations
reads it through this module, so that a term added once is left out everywhere:

  counted()        regular expression of the [zh] terms, for counting Simplified text
                   (scripts/mimo_pairs.py, mimo_phrases.py, mimo_names.py)
  zh_hit(hans)     a lexicon word (Simplified) that is never volunteered
  drop_hit(hans)   a lexicon word that is left out of the lexicon (slogans)
  ja_hit(surface)  a Japanese word that is never volunteered
  zh_terms()       the [zh] terms (without the = of a whole-word term)
  en_words()       English words (plain form: lower case, no hyphen, no trailing 's)
  en_hit(word)     an English word that is listed, or a listed word plus -s, -ic, -ist(s), -ism(s), -ite(s)
  en_pair_hit(l,r) an English word pair (left_right in [en]) whose right word is never offered after the left
  emoji_key(word)  a word that gets no emoji association

  python3 scripts/blocked_terms.py --client OUT   writes OUT, the list as the clients read it
                                                  (Sources/NeverVolunteer.swift): FNV-1a hashes, no text
"""
import functools
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIST = ROOT / "Resources/Lexicon/blocked-terms.txt"
SECTIONS = ("zh", "zh-keep", "zh-drop", "ja", "en", "emoji-key")


@functools.lru_cache(maxsize=None)
def sections(path: Path = LIST) -> dict:
    """{section: [terms]} in file order."""
    result = {name: [] for name in SECTIONS}
    current = None
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            current = line[1:-1]
            if current not in result:
                raise ValueError(f"{path.name}:{number}: unknown section {line}")
            continue
        if current is None:
            raise ValueError(f"{path.name}:{number}: a term before the first section")
        result[current].extend(line.split())
    return result


def _alternation(terms):
    return "|".join(sorted(map(re.escape, set(terms)), key=len, reverse=True))


@functools.lru_cache(maxsize=None)
def counted():
    """Pattern of the [zh] terms anywhere in a text (a whole-word term too: in running text there are no
    word boundaries), for leaving sentences out of the counts."""
    return re.compile(_alternation(t.lstrip("=") for t in sections()["zh"]))


@functools.lru_cache(maxsize=None)
def _zh():
    terms = sections()["zh"]
    anywhere = re.compile(_alternation(t for t in terms if not t.startswith("=")))
    whole = {t[1:] for t in terms if t.startswith("=")}
    return anywhere, whole, set(sections()["zh-keep"])


def zh_terms() -> list:
    """The [zh] terms in file order, a whole-word term without its =."""
    return [t.lstrip("=") for t in sections()["zh"]]


@functools.lru_cache(maxsize=None)
def zh_hit(hans: str) -> bool:
    """Whether a lexicon word (Simplified text) is on the never-volunteer list."""
    anywhere, whole, keep = _zh()
    return hans not in keep and (hans in whole or anywhere.search(hans) is not None)


@functools.lru_cache(maxsize=None)
def zh_whole_only(hans: str) -> bool:
    """Whether a never-volunteer word is on the list only as a whole word (=term): inside a longer
    word it is no term of the list."""
    anywhere, whole, keep = _zh()
    return hans not in keep and hans in whole and anywhere.search(hans) is None


@functools.lru_cache(maxsize=None)
def _drop():
    terms = sections()["zh-drop"]
    return re.compile("|".join(".*".join(map(re.escape, t.split("*"))) for t in terms)), set(terms)


def drop_hit(hans: str) -> bool:
    """Whether a lexicon word (Simplified text) is left out: it contains a [zh-drop] term and is not
    the term itself (the term stays a word; a slogan built on it does not)."""
    pattern, terms = _drop()
    return hans not in terms and pattern.search(hans) is not None


def _katakana(text: str) -> bool:
    return all("゠" <= ch <= "ヿ" for ch in text)


@functools.lru_cache(maxsize=None)
def _ja():
    terms = sections()["ja"]
    whole = {t for t in terms if _katakana(t)}
    anywhere = [t for t in terms if t not in whole]
    return re.compile(_alternation(anywhere)) if anywhere else None, whole


@functools.lru_cache(maxsize=None)
def ja_hit(surface: str) -> bool:
    """Whether a Japanese surface form is never volunteered (a katakana term matches the whole word only, so a longer word that starts with it is not hit)."""
    anywhere, whole = _ja()
    return surface in whole or (anywhere is not None and anywhere.search(surface) is not None)


def plain_form(word: str) -> str:
    """Lower case, without a trailing 's and hyphens (who're stays who're), as scripts/mimo_english.py compares."""
    return re.sub(r"['’]s$", "", word.lower()).replace("-", "")


@functools.lru_cache(maxsize=None)
def en_words() -> frozenset:
    """The listed English words (not the word pairs)."""
    return frozenset(plain_form(t) for t in sections()["en"] if "_" not in t)


@functools.lru_cache(maxsize=None)
def en_pairs() -> frozenset:
    """(left, right) of the [en] word pairs, plain forms."""
    return frozenset(tuple(plain_form(w) for w in t.split("_", 1)) for t in sections()["en"] if "_" in t)


# A listed word with one of these endings is listed too (communists, fascistic, maoist, stalinism, leninist).
EN_ENDINGS = ("s", "ic", "ist", "ists", "ism", "isms", "ite", "ites")


@functools.lru_cache(maxsize=None)
def en_hit(word: str) -> bool:
    plain = plain_form(word)
    words = en_words()
    return plain in words or any(plain.endswith(e) and plain[:-len(e)] in words for e in EN_ENDINGS)


def en_pair_hit(left: str, right: str) -> bool:
    return (plain_form(left), plain_form(right)) in en_pairs()


@functools.lru_cache(maxsize=None)
def _emoji_keys():
    return frozenset(sections()["emoji-key"])


def emoji_key(word: str) -> bool:
    """Whether a word (zh or ja) gets no emoji association of its own."""
    return word in _emoji_keys()


def fnv(text: str) -> str:
    """FNV-1a 64 of the UTF-8 bytes, 16 hex digits (NeverVolunteer.swift computes the same)."""
    h = 0xCBF29CE484222325
    for b in text.encode("utf-8"):
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{h:016x}"


def client_list() -> str:
    """The list for the clients: per section the FNV-1a hashes of its terms, so that the app carries no
    readable list. Sections: zh (a Simplified term anywhere in a text; [zh] and the plain [zh-drop] terms), zh-whole (=term: the whole text),
    zh-keep (whole texts that are not hit), ja (kanji terms, anywhere), ja-whole (katakana terms, a whole
    katakana run), en (whole words; the endings of EN_ENDINGS are tried by the client), emoji-key.
    `longest` is the longest term in characters (Unicode scalars), for the substrings a client hashes."""
    s = sections()
    ja_whole = [t for t in s["ja"] if _katakana(t)]
    parts = {
        "zh": [t for t in s["zh"] if not t.startswith("=")] + [t for t in s["zh-drop"] if "*" not in t],
        "zh-whole": [t[1:] for t in s["zh"] if t.startswith("=")],
        "zh-keep": s["zh-keep"],
        "ja": [t for t in s["ja"] if t not in ja_whole],
        "ja-whole": ja_whole,
        "en": sorted(en_words()),
        "emoji-key": s["emoji-key"],
    }
    longest = max(len(t) for name in ("zh", "ja") for t in parts[name])
    lines = ["# Generated by scripts/blocked_terms.py --client from Resources/Lexicon/blocked-terms.txt.",
             "# FNV-1a 64 (UTF-8) of each term; read by Sources/NeverVolunteer.swift.",
             f"longest {longest}",
             "endings " + " ".join(EN_ENDINGS)]
    for name, terms in parts.items():
        lines.append(f"[{name}]")
        lines.extend(sorted({fnv(t) for t in terms}))
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    import sys
    if len(sys.argv) == 3 and sys.argv[1] == "--client":
        Path(sys.argv[2]).write_text(client_list(), encoding="utf-8")
    else:
        for name, terms in sections().items():
            print(f"[{name}] {len(terms)} terms")
