#!/usr/bin/env python3
"""MTGJSON AtomicCards -> a Magic: The Gathering card-semantics knowledge graph (.nt).

The Oracle-level card pool (one node per card *concept*, per the plan's identity model)
as a richly interlinked graph: cards <-> colors, card types, subtypes, supertypes,
keywords, formats (legalities), sets (printings), mana and Oracle text. Card types are
modelled as OWL classes (Creature rdfs:subClassOf Card, and each card rdf:type its
types) so the schema pyramid becomes a type zoom and the reasoner has something to chew
on; Oracle text goes in a TEXT_INDEX so you can search cards by what they *do*.

Sources: MTGJSON (MIT) AtomicCards; vocabulary inspired by cmdoret/mtg_ontology (GPL-3.0).
Neutral vocab on w3id.org/rete + schema.org/Dublin Core."""
import json, re, sys, unicodedata
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC  = REPO / "data" / "mtg" / "raw" / "AtomicCards.json"
OUT  = REPO / "data" / "mtg" / "mtg.nt"

MTG    = "https://w3id.org/rete/mtg#"
B      = "https://w3id.org/rete/mtg/"
SCHEMA = "http://schema.org/"
DCT    = "http://purl.org/dc/terms/"
MTGO   = "https://cmdoret.net/mtg_ontology/"      # Cyril Matthey-Doret's structural classes
FANDOM = "https://mtg.fandom.com/wiki/"           # ...and his wiki-linked concept classes

# bridges to cmdoret/mtg_ontology (the plan's semantic foundation)
TYPE_EQUIV = {"Creature", "Instant", "Sorcery", "Land", "Artifact", "Enchantment"}  # ≡ mtgo:<same>
COLOR_FANDOM = {"W": "white", "U": "blue", "B": "black", "R": "red", "G": "green", "C": "colorless"}
EVERGREEN = {"deathtouch", "defender", "double_strike", "first_strike", "flash", "flying",
             "haste", "hexproof", "indestructible", "lifelink", "menace", "protection",
             "reach", "trample", "vigilance", "ward"}
RDF    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
TYPE   = RDF + "type"
RDFS   = "http://www.w3.org/2000/01/rdf-schema#"
OWL    = "http://www.w3.org/2002/07/owl#"
XSD    = "http://www.w3.org/2001/XMLSchema#"

COLOR_NAME = {"W": "White", "U": "Blue", "B": "Black", "R": "Red", "G": "Green", "C": "Colorless"}

def slugify(name):
    s = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
    s = re.sub(r"[^A-Za-z0-9]+", "-", s).strip("-").lower()
    return s or "card"

def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "").replace("\t", "\\t"))

def main():
    data = json.load(open(SRC, encoding="utf-8"))["data"]
    img_path = REPO / "data" / "mtg" / "raw" / "oracle_images.json"
    images = json.load(open(img_path, encoding="utf-8")) if img_path.exists() else {}
    set_path = REPO / "data" / "mtg" / "raw" / "SetList.json"
    set_meta = {}
    if set_path.exists():
        for s in json.load(open(set_path, encoding="utf-8"))["data"]:
            set_meta[s["code"]] = s
    out = open(OUT, "w", encoding="utf-8")
    W = out.write
    def iri(s, p, o): W(f"<{s}> <{p}> <{o}> .\n")
    def lit(s, p, v, dt=None): W(f'<{s}> <{p}> "{esc(str(v))}"' + (f"^^<{dt}>" if dt else "") + " .\n")

    # collect vocab nodes to emit once at the end
    colors, ctypes, subtypes, supertypes, keywords, formats, sets = set(), set(), set(), set(), set(), set(), set()
    seen_slug = {}
    n_cards = 0

    for name, faces in data.items():
        slug = slugify(name)
        if slug in seen_slug:
            seen_slug[slug] += 1; slug = f"{slug}-{seen_slug[slug]}"
        else:
            seen_slug[slug] = 1
        c = B + "card/" + slug
        f0 = faces[0]

        # aggregate across faces (multi-face cards)
        def collect(key):
            vals = []
            for f in faces:
                for v in (f.get(key) or []):
                    if v not in vals: vals.append(v)
            return vals
        cols   = collect("colors")
        cid    = collect("colorIdentity")
        types  = collect("types")
        subs   = collect("subtypes")
        supers = collect("supertypes")
        kws    = collect("keywords")
        produced = collect("producedMana")
        text = " // ".join(f.get("text", "") for f in faces if f.get("text"))
        legal = {}
        for f in faces:
            legal.update(f.get("legalities") or {})
        prints = collect("printings") or f0.get("printings") or []

        iri(c, TYPE, MTG + "Card")
        lit(c, SCHEMA + "name", name)
        oid = (f0.get("identifiers") or {}).get("scryfallOracleId")
        if oid and oid in images:
            iri(c, MTG + "image", images[oid]); iri(c, SCHEMA + "image", images[oid])
        for ty in types:
            iri(c, TYPE, MTG + ty.replace(" ", "")); ctypes.add(ty)
        mc = next((f.get("manaCost") for f in faces if f.get("manaCost")), None)
        if mc: lit(c, MTG + "manaCost", mc)
        if f0.get("manaValue") is not None:
            mv = f0["manaValue"]; lit(c, MTG + "manaValue", int(mv) if mv == int(mv) else mv, XSD + "decimal")
        if text: lit(c, MTG + "oracleText", text)
        for k, pred in (("power", "power"), ("toughness", "toughness"), ("loyalty", "loyalty"), ("defense", "defense")):
            v = next((f.get(k) for f in faces if f.get(k) is not None), None)
            if v is not None: lit(c, MTG + pred, v)
        if f0.get("edhrecRank") is not None:
            lit(c, MTG + "edhrecRank", int(f0["edhrecRank"]), XSD + "integer")
        if any(f.get("isReserved") for f in faces):
            lit(c, MTG + "reserved", "true", XSD + "boolean")
        for col in cols:      iri(c, MTG + "color", B + "color/" + col); colors.add(col)
        for col in cid:       iri(c, MTG + "colorIdentity", B + "color/" + col); colors.add(col)
        for col in produced:  iri(c, MTG + "producesMana", B + "color/" + col); colors.add(col)
        for st in subs:       iri(c, MTG + "subtype", B + "subtype/" + slugify(st)); subtypes.add(st)
        for st in supers:     iri(c, MTG + "supertype", B + "supertype/" + slugify(st)); supertypes.add(st)
        for kw in kws:        iri(c, MTG + "keyword", B + "keyword/" + slugify(kw)); keywords.add(kw)
        for fmt, status in legal.items():
            pred = {"Legal": "legalIn", "Restricted": "restrictedIn", "Banned": "bannedIn"}.get(status)
            if pred: iri(c, MTG + pred, B + "format/" + fmt); formats.add(fmt)
        for s in prints:      iri(c, MTG + "printedIn", B + "set/" + s); sets.add(s)
        # rulings (dedup across faces) — the plan's rule-evidence layer; text feeds the TEXT_INDEX
        seen_r = set(); ri = 0
        for f in faces:
            for r in (f.get("rulings") or []):
                key = (r.get("date"), r.get("text"))
                if key in seen_r or not r.get("text"): continue
                seen_r.add(key)
                rn = B + f"ruling/{slug}/{ri}"; ri += 1
                iri(c, MTG + "ruling", rn); iri(rn, TYPE, MTG + "Ruling")
                if r.get("date"): lit(rn, DCT + "date", r["date"], XSD + "date")
                lit(rn, RDFS + "comment", r["text"])
        n_cards += 1

    # vocab: card-type classes (TBox), colors, subtypes, supertypes, keywords, formats, sets
    iri(MTG + "Card", TYPE, OWL + "Class"); lit(MTG + "Card", RDFS + "label", "Card")
    for ty in sorted(ctypes):
        cl = MTG + ty.replace(" ", "")
        iri(cl, TYPE, OWL + "Class"); iri(cl, RDFS + "subClassOf", MTG + "Card"); lit(cl, RDFS + "label", ty)
    for col in sorted(colors):
        n = B + "color/" + col; iri(n, TYPE, MTG + "Color"); lit(n, RDFS + "label", COLOR_NAME.get(col, col))
    for st in sorted(subtypes):
        n = B + "subtype/" + slugify(st); iri(n, TYPE, MTG + "Subtype"); lit(n, RDFS + "label", st)
    for st in sorted(supertypes):
        n = B + "supertype/" + slugify(st); iri(n, TYPE, MTG + "Supertype"); lit(n, RDFS + "label", st)
    for kw in sorted(keywords):
        n = B + "keyword/" + slugify(kw); iri(n, TYPE, MTG + "Keyword"); lit(n, RDFS + "label", kw)
    for fmt in sorted(formats):
        n = B + "format/" + fmt; iri(n, TYPE, MTG + "Format"); lit(n, RDFS + "label", fmt.capitalize())
    for s in sorted(sets):
        n = B + "set/" + s; iri(n, TYPE, MTG + "Set")
        meta = set_meta.get(s)
        if meta:
            lit(n, SCHEMA + "name", meta["name"]); lit(n, RDFS + "label", meta["name"])
            if meta.get("releaseDate"): lit(n, MTG + "releaseDate", meta["releaseDate"], XSD + "date")
            if meta.get("type"): lit(n, MTG + "setType", meta["type"])
            if meta.get("totalSetSize") is not None: lit(n, MTG + "setSize", meta["totalSetSize"], XSD + "integer")
        else:
            lit(n, RDFS + "label", s)

    # --- bridges to cmdoret/mtg_ontology (the plan's semantic foundation) ---
    for ty in sorted(ctypes):
        if ty in TYPE_EQUIV:
            iri(MTG + ty.replace(" ", ""), OWL + "equivalentClass", MTGO + ty)
    for col in sorted(colors):
        if col in COLOR_FANDOM:
            n = B + "color/" + col
            iri(n, RDFS + "seeAlso", FANDOM + COLOR_FANDOM[col]); iri(n, TYPE, MTGO + "Color")
    for kw in sorted(keywords):
        key = slugify(kw).replace("-", "_")
        if key in EVERGREEN:
            n = B + "keyword/" + slugify(kw)
            iri(n, RDFS + "seeAlso", FANDOM + key); iri(n, TYPE, MTGO + "AbilityKeyword")

    out.close()
    print(f"cards: {n_cards:,} | colors {len(colors)}, types {len(ctypes)}, subtypes {len(subtypes)}, "
          f"supertypes {len(supertypes)}, keywords {len(keywords)}, formats {len(formats)}, sets {len(sets)}")
    print(f"-> {OUT}  ({OUT.stat().st_size/1e6:.1f} MB)")

if __name__ == "__main__":
    main()
