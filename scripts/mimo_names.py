"""Names and terms of many fields, from Wikidata (CC0): anime, manga and games and their
characters, films and books, medicine, chemistry, animals and plants, food, computing,
companies and products, places and stations, space, music, sports, study and work.

The word lists have few of them (鬼滅之刃, 芙莉蓮, 布洛芬, 高鐵左營站), and people type them
every day. compile-native-lexicon.py adds the names of DataSources/mimo-names/names.tsv
as words, read character by character, at a weight below everyday words. Real persons are
not collected.

  python3 scripts/mimo_names.py fetch
      Asks query.wikidata.org for the items of the classes below that have a Chinese label,
      with the number of Wikipedia editions that have an article on them (a measure of how
      well known the item is). Raw answers go to .build/names.
  python3 scripts/mimo_names.py build CONVERTER
      CONVERTER is .build/convert-script. Keeps labels of 2 to 8 Han characters without a
      blocked term (Resources/Lexicon/blocked-terms.txt), in the Taiwan and the mainland
      wording when they differ, and writes names.tsv (Traditional, Simplified, editions)
      and SOURCE.json.
"""
import hashlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path

from mimo_pairs import blocked_terms

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "DataSources/mimo-names"
RAW = ROOT / ".build/names"
ENDPOINT = "https://query.wikidata.org/sparql"
# Field -> [(how an item belongs, Wikidata class, fewest Wikipedia editions)].
# "is": instance of the class; "kind": the class or one of its subclasses; "is kind": instance of
# the class or of a subclass. No class of people (real persons are not collected), none of
# politics, war or religion.
GROUPS = {
    "anime": [("is", c, 2) for c in "Q63952888 Q21198342 Q20650540 Q1107 Q747381 Q104213567 Q8274".split()],
    "characters": [("is", c, 2) for c in "Q15711870 Q80447738 Q87576284 Q1569167 Q15773347 Q95074".split()],
    "games": [("is", "Q7889", 6), ("is", "Q7058673", 6)],
    "screen": [("is", "Q11424", 40), ("is", "Q5398426", 20), ("is", "Q24856", 20), ("is", "Q202866", 20)],
    "books": [("is", "Q7725634", 25), ("is", "Q47461344", 25), ("is", "Q8261", 25)],
    "medicine": [("kind", "Q12136", 10), ("kind", "Q169872", 10), ("is kind", "Q12140", 8), ("kind", "Q796194", 10),
                 ("kind", "Q4936952", 15), ("is", "Q113145171", 25), ("is", "Q11344", 30)],
    "food": [("kind", "Q746549", 8), ("kind", "Q40050", 10), ("kind", "Q3314483", 10), ("kind", "Q11004", 10), ("kind", "Q25403900", 10)],
    "life": [("is", "Q55983715", 20), ("is", "Q39367", 8), ("is", "Q43577", 5), ("is", "Q16521", 90)],
    "computing": [("is", "Q9143", 10), ("is", "Q7397", 20), ("is", "Q9135", 10), ("is", "Q235557", 10), ("kind", "Q8366", 10),
                  ("is", "Q35127", 25), ("is", "Q15836568", 10)],
    "companies": [("is", "Q4830453", 25), ("is", "Q891723", 25), ("is", "Q431289", 15), ("is", "Q46970", 25), ("is", "Q22687", 25)],
    "products": [("is", "Q3231690", 10)],
    "places": [("is", "Q570116", 20), ("is", "Q33506", 25), ("is", "Q8502", 30), ("is", "Q4022", 35), ("is", "Q23397", 30),
               ("is", "Q23442", 35), ("is", "Q46169", 25), ("is", "Q515", 60), ("is", "Q5119", 40), ("is", "Q50337", 10),
               ("is", "Q494721", 15), ("is", "Q55488", 8), ("is", "Q928830", 8), ("is", "Q1248784", 25)],
    "space": [("is", "Q634", 20), ("is", "Q8928", 20), ("is", "Q523", 30), ("is", "Q318", 25), ("is", "Q2537", 25), ("is", "Q3863", 40)],
    "music": [("kind", "Q34379", 10), ("is", "Q188451", 15)],
    "sports": [("is", "Q31629", 15), ("kind", "Q349", 15), ("is", "Q476028", 40)],
    "study": [("is", "Q11862829", 20), ("is", "Q3918", 40), ("is", "Q65943", 15), ("kind", "Q107715", 15), ("kind", "Q47574", 15)],
    "work": [("is kind", "Q28640", 10)],
    "things": [("kind", "Q39546", 15), ("kind", "Q11460", 12), ("is", "Q7946", 25), ("kind", "Q42889", 20)],
    "culture": [("is", "Q132241", 15), ("is", "Q1445650", 15), ("is", "Q7366", 40)],
}
PATHS = {"is": "wdt:P31", "kind": "wdt:P279*", "is kind": "wdt:P31/wdt:P279*"}
LANGUAGES = ("zh-tw", "zh-hant", "zh", "zh-cn", "zh-hans")
NAME = re.compile(r"[㐀-䶿一-鿿]{2,8}")


def query(how: str, item_class: str, least: int) -> str:
    labels = "\n".join(f'  OPTIONAL {{ ?item rdfs:label ?l{i} FILTER(LANG(?l{i}) = "{lang}") }}' for i, lang in enumerate(LANGUAGES))
    return (f"SELECT DISTINCT ?item ?links {' '.join(f'?l{i}' for i in range(len(LANGUAGES)))} WHERE {{\n"
            f"  ?item {PATHS[how]} wd:{item_class} ; wikibase:sitelinks ?links .\n  FILTER(?links >= {least})\n{labels}\n}}")


def answered(path: Path) -> bool:
    if not path.exists():
        return False
    text = path.read_text(encoding="utf-8", errors="replace")
    return text.startswith("?item") and "Exception" not in text and text.endswith("\n")


def fetch():
    """One query per class; the service stops a query after a minute, and a class it cannot answer is left out."""
    RAW.mkdir(parents=True, exist_ok=True)
    for field, classes in GROUPS.items():
        for how, item_class, least in classes:
            path = RAW / f"{field}-{item_class}.tsv"
            for attempt in range(2):
                if answered(path):
                    break
                subprocess.run(["curl", "-sS", "--http1.1", "--max-time", "90", "-G", ENDPOINT, "--data-urlencode", "query=" + query(how, item_class, least),
                                "-H", "Accept: text/tab-separated-values", "-H", "User-Agent: lexicon-build/1.0", "-o", str(path)])
                time.sleep(3)
            if answered(path):
                print(path.stem, sum(1 for _ in open(path, encoding="utf-8")) - 1, "rows", flush=True)
            else:
                path.unlink(missing_ok=True)
                print(path.stem, "no answer, left out", flush=True)


def convert(converter: str, words: list[str], traditional: bool) -> dict:
    out = subprocess.run([converter] + (["--traditional"] if traditional else []), input="\n".join(words) + "\n",
                         text=True, capture_output=True, check=True).stdout.splitlines()
    return dict(zip(words, out))


def build(converter: str):
    blocked = blocked_terms()
    editions, items = {}, {}
    for path in sorted(RAW.glob("*.tsv")):
        name = path.stem.split("-")[0]
        with open(path, encoding="utf-8") as f:
            next(f)
            for line in f:
                fields = line.rstrip("\n").split("\t")
                links = int(re.sub(r"\D.*", "", fields[1].strip('"')) or 0)
                items[name] = items.get(name, 0) + 1
                for label in fields[2:]:
                    text = label.split('"')[1] if label.startswith('"') else ""
                    if NAME.fullmatch(text):
                        editions[text] = max(editions.get(text, 0), links)
    labels = sorted(editions)
    simplified = convert(converter, labels, False)
    traditional = convert(converter, labels, True)
    rows = {}
    for label in labels:
        hans = simplified[label]
        hant = label if hans != label else traditional[label]
        if not blocked.search(hans):
            rows[(hant, hans)] = max(rows.get((hant, hans), 0), editions[label])
    DATA.mkdir(parents=True, exist_ok=True)
    with open(DATA / "names.tsv", "w", encoding="utf-8") as f:
        f.write("# name in Traditional and in Simplified characters, Wikipedia editions with an article (scripts/mimo_names.py)\n")
        for (hant, hans), links in sorted(rows.items(), key=lambda row: (-row[1], row[0])):
            f.write(f"{hant}\t{hans}\t{links}\n")
    manifest = {
        "name": "mimo-names",
        "description": "Names and terms with a Chinese label on Wikidata (works, characters, medicine, chemistry, life, food, computing, companies, places and more; no persons); made by scripts/mimo_names.py and added to the lexicon at build time.",
        "license": "CC0-1.0 (Wikidata)",
        "source": {"endpoint": ENDPOINT, "fetched": time.strftime("%Y-%m-%d"),
                   "classes": {field: [f"{how} {c}, {least}+ editions" for how, c, least in classes] for field, classes in GROUPS.items()},
                   "answered": sorted(path.stem for path in RAW.glob("*.tsv")), "rows": items},
        "names": len(rows),
        "files": {"names.tsv": hashlib.sha256((DATA / "names.tsv").read_bytes()).hexdigest()},
    }
    (DATA / "SOURCE.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(len(rows), "names")


def names():
    """Rows of names.tsv: (Traditional, Simplified, editions)."""
    path = DATA / "names.tsv"
    if not path.exists():
        return []
    if hashlib.sha256(path.read_bytes()).hexdigest() != json.loads((DATA / "SOURCE.json").read_text())["files"]["names.tsv"]:
        raise ValueError("DataSources/mimo-names/names.tsv changed; make it with scripts/mimo_names.py build")
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.startswith("#"):
            hant, hans, links = line.split("\t")
            rows.append((hant, hans, int(links)))
    return rows


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "fetch":
        fetch()
    elif len(sys.argv) == 3 and sys.argv[1] == "build":
        build(sys.argv[2])
    else:
        sys.exit(__doc__)
