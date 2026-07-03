#!/usr/bin/env python3
"""Convert the HOLLIS for Archival Discovery CSV export of the Charles S.
Peirce papers finding aid (hou02614, Houghton Library, MS Am 1632) into an
N-Triples graph.

Source: data/peirce/hou02614.csv -- the CSV export the catalog page itself
offers (https://hollisarchives.lib.harvard.edu/catalog/hou02614), 2,527
components. No crawling: one download of the offered export.

Component IRI = its own catalog page
https://hollisarchives.lib.harvard.edu/catalog/{dbnum} (clickable permalink).

Entities: Collection, Series, Subseries, Component/File/Item, Box, Person.
The correspondence titles are regular ("Abbot, Francis Ellington.  One letter
to Charles S. Peirce, January 5, 1895") -> a directed correspondent network
(a:sender / a:addressee, a:letterCount). Manuscript titles split into short
title, genre and date text; years land in a:startYear/a:endYear (xsd:integer)
for range queries. Robin-arrangement numbers ("MS Am 1632, (104)") land in
a:msNumber.
"""
import csv
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "data", "peirce", "hou02614.csv")
OUT = os.path.join(ROOT, "data", "peirce", "peirce.nt")

CAT = "https://hollisarchives.lib.harvard.edu/catalog/"
HID = "https://id.lib.harvard.edu/ead/hou02614/"      # minted ids: box/N, person/slug
A = "https://hollisarchives.lib.harvard.edu/ontology#"
DCT = "http://purl.org/dc/terms/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"
INT = XSD + "integer"

COLL = CAT + "hou02614"
PEIRCE = HID + "person/peirce-charles-sanders"

TYPE_CLASS = {
    "Series": A + "Series",
    "Subseries": A + "Subseries",
    "File": A + "File",
    "Item": A + "Item",
    "Unspecified": A + "Component",
}

_slug = re.compile(r"[^a-z0-9]+")
_ctrl = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f   ]")
MONTHS = ("January", "February", "March", "April", "May", "June", "July",
          "August", "September", "October", "November", "December")
# a date expression starting right after a comma: "circa 1903",
# "September 21, 1874", "1897-1910", "undated", "July 8, circa 1872-May 10, 1892"
DATE_TAIL = re.compile(
    r",\s*((?:circa\s+|c\.\s*)?(?:%s|\d{4}|undated)[^:]*?)\s*\.?\s*$" % "|".join(MONTHS))
YEAR = re.compile(r"\b(1[6-9]\d{2})(?:\s*-\s*(\d{2,4}))?\b")
PEIRCE_PREFIX = re.compile(r"^Peirce,\s+Charles\s+S\.\s+\(Charles\s+Sanders\),\s+1839-1914\.\s*")
LETTER_PHRASE = re.compile(
    r"\b(letter|letters|card|cards|note|notes|telegram|telegrams|postcard|postcards)\b", re.I)

WORDNUM = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6,
           "seven": 7, "eight": 8, "nine": 9, "ten": 10, "eleven": 11,
           "twelve": 12, "thirteen": 13, "fourteen": 14, "fifteen": 15,
           "sixteen": 16, "seventeen": 17, "eighteen": 18, "nineteen": 19}
WORDTEN = {"twenty": 20, "thirty": 30, "forty": 40, "fifty": 50,
           "sixty": 60, "seventy": 70, "eighty": 80, "ninety": 90}
# correspondent-name separator: double space, " : ", or ". " right before a
# letter-count/letter-kind word ("Curtis, Matthew Mattoon. Letter draft ...")
CORR_SPLIT = re.compile(
    r"\s{2,}|\s+:\s+|\.\s+(?=(?:%s|A |An |Letter|Note|Card|Telegram|Postal|Correspondence|Part|\d))"
    % "|".join(w.capitalize() for w in list(WORDNUM) + list(WORDTEN)))


def slug(s):
    s = _slug.sub("-", (s or "").strip().lower()).strip("-")
    return s or "x"


def clean(s):
    return re.sub(r"\s+", " ", _ctrl.sub(" ", s or "")).strip()


def esc(s):
    s = _ctrl.sub(" ", str(s))
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def ci(u):
    u = re.sub(r"\s+", "", str(u))
    return u if u and "<" not in u and ">" not in u and '"' not in u else None


class W:
    def __init__(s, fh): s.fh = fh; s.n = 0
    def iri(s, a, p, b):
        a, b = ci(a), ci(b)
        if a and b: s.fh.write(f"<{a}> <{p}> <{b}> .\n"); s.n += 1
    def lit(s, a, p, o, dt=None):
        a = ci(a)
        if not a or o is None or o == "": return
        o = esc(o)
        if dt: s.fh.write(f'<{a}> <{p}> "{o}"^^<{dt}> .\n')
        else: s.fh.write(f'<{a}> <{p}> "{o}" .\n')
        s.n += 1


def word_count(text):
    """'One'->1, 'Sixty-six'->66, '105'->105, 'Two hundred and sixteen'->216
    from the start of a phrase."""
    m = re.match(r"\s*(\d+)\b", text)
    if m: return int(m.group(1))
    m = re.match(r"\s*([A-Za-z]+)(?:\s+hundred(?:\s+and\s+([a-z]+))?)?(?:-([a-z]+))?\b",
                 text, re.I)
    if not m: return None
    w1, rest, w3 = (m.group(1) or "").lower(), (m.group(2) or "").lower(), (m.group(3) or "").lower()
    if "hundred" in m.group(0).lower():
        if w1 not in WORDNUM: return None
        n = WORDNUM[w1] * 100
        if rest:
            n += WORDNUM.get(rest) or WORDTEN.get(rest, 0)
        if w3: n += WORDNUM.get(w3, 0)
        return n
    if w1 in WORDNUM and not w3: return WORDNUM[w1]
    if w1 in WORDTEN: return WORDTEN[w1] + WORDNUM.get(w3, 0)
    if w1 in ("a", "an"): return 1
    return None


def parse_dates(title):
    """-> (date_text, start_year, end_year) from a component title tail."""
    m = DATE_TAIL.search(title)
    if not m: return None, None, None
    dt = m.group(1).strip().rstrip(".").strip()
    if dt.lower() == "undated": return "undated", None, None
    years = []
    for y in YEAR.finditer(dt):
        y1 = int(y.group(1))
        years.append(y1)
        if y.group(2):
            y2 = y.group(2)
            y2 = int(y2) if len(y2) == 4 else int(str(y1)[:4 - len(y2)] + y2)
            if 1600 < y2 < 2000: years.append(y2)
    years = [y for y in years if 1650 <= y <= 1960]
    if not years: return dt, None, None
    return dt, min(years), max(years)


def main():
    rows = list(csv.reader(open(SRC, encoding="utf-8-sig")))
    meta = {r[0]: r[1] for r in rows[:5]}
    data = rows[6:]

    # dedupe by database number, keep first occurrence
    seen, comps = set(), []
    for r in data:
        if len(r) < 17 or not r[0].strip() or r[0] in seen: continue
        seen.add(r[0]); comps.append(r)

    # series/subseries title -> id, for parent resolution via Level 1..5
    tmap = {}
    for r in comps:
        if r[7] in ("Series", "Subseries"):
            t = clean(r[1])
            if t in tmap:
                print(f"  WARN duplicate series title: {t}")
            tmap[t] = CAT + r[0]

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    fh = open(OUT, "w", encoding="utf-8", newline="\n")
    w = W(fh)

    # --- collection node ---
    w.iri(COLL, RDF + "type", A + "Collection")
    w.lit(COLL, DCT + "title", clean(meta.get("Collection Title", "")))
    w.lit(COLL, RDFS + "label", "Charles S. Peirce papers")
    w.lit(COLL, A + "callNumber", meta.get("Call Number", ""))
    w.lit(COLL, A + "eadId", meta.get("EAD ID", ""))
    w.lit(COLL, DCT + "date", meta.get("Collection Dates", ""))
    w.lit(COLL, A + "startYear", "1787", INT)
    w.lit(COLL, A + "endYear", "1951", INT)
    w.lit(COLL, DCT + "abstract",
          "Papers of philosopher, logician, scientist, and the founder of "
          "pragmatism, Charles S. Peirce. Also includes Peirce family correspondence.")
    w.lit(COLL, DCT + "extent", "74 linear feet (166 boxes, 1 volume, 1 bundle)")
    w.lit(COLL, A + "repository", "Houghton Library, Harvard University")
    w.iri(COLL, DCT + "creator", PEIRCE)
    w.iri(COLL, RDFS + "seeAlso", "https://id.lib.harvard.edu/ead/hou02614/catalog")

    w.iri(PEIRCE, RDF + "type", A + "Person")
    w.lit(PEIRCE, RDFS + "label", "Charles Sanders Peirce")
    w.lit(PEIRCE, A + "personDates", "1839-1914")
    w.iri(PEIRCE, OWL + "sameAs", "http://www.wikidata.org/entity/Q187520")

    persons, boxes = {PEIRCE: None}, set()

    def person(name):
        name = clean(name).strip(" ,.")
        if not name or len(name) < 2: return None
        if re.match(r"^Peirce,\s+Charles\s+S\b", name) or name == "Charles S. Peirce":
            return PEIRCE
        iri = HID + "person/" + slug(name)
        if iri not in persons:
            persons[iri] = name
            w.iri(iri, RDF + "type", A + "Person")
            w.lit(iri, RDFS + "label", name)
        return iri

    n_letters = n_dated = n_digital = 0
    for r in comps:
        (dbnum, title, _cd, _sy, _ey, ident, container, ctype, _creator,
         digital, access, phys) = r[:12]
        levels = [clean(x) for x in r[12:17] if clean(x)]
        s = CAT + dbnum
        raw = _ctrl.sub(" ", title).strip()   # keeps the double-space separators
        title = clean(title)
        cls = TYPE_CLASS.get(ctype, A + "Component")
        w.iri(s, RDF + "type", cls)
        w.lit(s, DCT + "title", title)

        # position in the finding aid = numeric part of the component id
        m = re.search(r"c(\d+)$", dbnum)
        if m: w.lit(s, A + "order", str(int(m.group(1))), INT)

        # hierarchy: deepest ancestor level that resolves, else the collection
        parent = COLL
        for lv in reversed(levels):
            if lv in tmap and tmap[lv] != s:
                parent = tmap[lv]; break
        w.iri(s, DCT + "isPartOf", parent)

        # identifier + Robin-arrangement number "MS Am 1632, (104)"
        ident = clean(ident)
        if ident:
            w.lit(s, DCT + "identifier", ident)
            m = re.search(r"\((\d+)\)", ident)
            if m: w.lit(s, A + "msNumber", m.group(1), INT)

        # container
        container = clean(container)
        m = re.match(r"^Box\s+(\d+)$", container)
        if m:
            b = HID + "box/" + m.group(1)
            if b not in boxes:
                boxes.add(b)
                w.iri(b, RDF + "type", A + "Box")
                w.lit(b, RDFS + "label", container)
                w.lit(b, A + "boxNumber", m.group(1), INT)
            w.iri(s, A + "box", b)
        elif container:
            w.lit(s, A + "containerNote", container)

        if digital.strip():
            w.iri(s, A + "digitalContent", digital.strip())
            n_digital += 1
        w.lit(s, A + "accessNote", clean(access))
        w.lit(s, A + "physicalDescription", clean(phys))

        if ctype in ("Series", "Subseries"):
            w.lit(s, RDFS + "label", title)
            continue

        # --- title parsing (on the raw title: separators are space runs) ---
        rest_raw = raw
        pm = PEIRCE_PREFIX.match(rest_raw)
        if pm:
            w.iri(s, DCT + "creator", PEIRCE)
            rest_raw = rest_raw[pm.end():]
        rest = clean(rest_raw)

        date_text, y1, y2 = parse_dates(rest)
        if date_text:
            w.lit(s, A + "dateText", date_text)
            if y1:
                w.lit(s, A + "startYear", str(y1), INT)
                w.lit(s, A + "endYear", str(y2), INT)
                n_dated += 1

        # "short title : genre, date" for manuscripts
        parts = [clean(p) for p in re.split(r"\s+:\s+", rest_raw, 1)]
        short = clean(parts[0])
        if date_text and short.endswith(date_text):
            short = clean(short[: -len(date_text)].rstrip(" ,."))
        if len(parts) == 2:
            genre = clean(parts[1])
            if date_text and genre.endswith(date_text):
                genre = clean(genre[: -len(date_text)].rstrip(" ,."))
            genre = genre.strip(" .,")
            if genre and not LETTER_PHRASE.search(genre):
                w.lit(s, A + "genre", genre)

        # --- correspondence network ---
        in_corr = any(lv.startswith("II.") for lv in levels)
        lm = LETTER_PHRASE.search(rest)
        if in_corr and lm:
            # correspondent = text before the separator (double space / " : " /
            # ". " right before the letter phrase)
            head = [clean(p) for p in CORR_SPLIT.split(rest_raw, 1) if p is not None]
            corr_p = None
            if (len(head) == 2 and 2 < len(head[0].strip(" ,.")) < 90 and not pm
                    and not LETTER_PHRASE.search(head[0])):
                corr_p = person(head[0])
                if corr_p: w.iri(s, A + "correspondent", corr_p)
            phrase = head[1] if len(head) == 2 else rest
            if date_text and phrase.endswith(date_text):
                phrase = phrase[: -len(date_text)].rstrip(" ,.")
            w.lit(s, A + "letterNote", clean(phrase))
            cnt = word_count(phrase)
            if cnt: w.lit(s, A + "letterCount", str(cnt), INT)
            n_letters += 1
            # direction
            # a plausible name = at least two words (avoids bare first names)
            NAME = r"(Charles S\. Peirce|[A-Z][\w.'\-]+(?:\s+[\w.'\-]+){1,5}?)(?=,|\s{2,}|$)"
            tm = re.search(r"\b(?:to)\s+" + NAME, phrase)
            fm = re.search(r"\b(?:from)\s+" + NAME, phrase)
            if tm:
                addr = person(tm.group(1))
                if addr:
                    w.iri(s, A + "addressee", addr)
                    if corr_p and addr != corr_p: w.iri(s, A + "sender", corr_p)
            if fm:
                sender = person(fm.group(1))
                if sender:
                    w.iri(s, A + "sender", sender)
                    if corr_p and sender != corr_p and not tm:
                        w.iri(s, A + "addressee", corr_p)
        elif not pm:
            # non-Peirce, non-correspondence: try a "Surname, First." creator prefix
            cm = re.match(r"^([A-Z][\w'\-]+,\s+[A-Z][\w.'\- ()]{2,60}?)\.\s{2,}", rest_raw)
            if cm:
                p = person(cm.group(1))
                if p: w.iri(s, DCT + "creator", p)

        # display label = short title if we got one, else the full title
        w.lit(s, RDFS + "label", short if 3 <= len(short) <= 200 else title)

    fh.close()
    print(f"components={len(comps)} triples={w.n} persons={len(persons)} "
          f"boxes={len(boxes)} letters={n_letters} year-ranged={n_dated} digital={n_digital}")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
