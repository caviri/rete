#!/usr/bin/env python3
"""Aggregate Spanish Civil War memoria-historica OPEN DATA into one N-Triples graph.

Sources (all CC-BY-style open data, downloaded by the sibling shell step):
  PERSONS (mc:Victim)
    cat_reparacio.csv       Catalonia - reparacio juridica de victimes del franquisme (69,834)
    cat_desaparecidos.csv   Catalonia - desapareguts de la Guerra Civil (8,339)
    euskadi_victimas.json   Euskadi   - victimas mortales de la Guerra Civil (21,369)
  MASS GRAVES (mc:MassGrave)
    cat_fosas.csv           Catalonia (1,027, with WGS84 lat/long)
    and_fosas.csv           Andalucia (977, narrative + victim counts)
    cyl_fosas.csv           Castilla y Leon (876, ';'-delimited, BOM)
    val_fosas.csv           C. Valenciana (529; WKT is UTM-30N, coords skipped)

Model (mc: = https://memoria.rete/ns#):
  persona/<src>/<id> a mc:Victim ; rdfs:label <name> ; mc:sex ; mc:age ;
     mc:bornInMunicipality ; mc:bornInProvince -> provincia/<slug> ;
     mc:residedInMunicipality ; mc:residedInProvince -> ... ;
     mc:deathDate ; mc:deathPlace ; mc:cause ; mc:procedure ; mc:sentence ;
     mc:executed (xsd:boolean) ; mc:profession ; mc:militaryUnit ; mc:burialPlace ;
     dcterms:source
  fosa/<src>/<id> a mc:MassGrave ; rdfs:label <title> ; mc:municipality ;
     mc:province -> provincia/<slug> ; mc:victimCount (int) ; mc:status ;
     mc:category ; mc:narrative ; mc:date ; mc:repressorSide ;
     geo:lat / geo:long (WGS84) ; geo:asWKT ; foaf:page ; dcterms:source
  provincia/<slug> a mc:Province ; rdfs:label
"""
import csv, json, os, re, sys, unicodedata

csv.field_size_limit(10_000_000)
HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(HERE, "data", "memoria", "sources")
OUT = os.path.join(HERE, "data", "memoria", "memoria.nt")

B = "https://memoria.rete/"
MC = B + "ns#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
FOAF = "http://xmlns.com/foaf/0.1/"
GEO = "http://www.w3.org/2003/01/geo/wgs84_pos#"
GSP = "http://www.opengis.net/ont/geosparql#"
XSD = "http://www.w3.org/2001/XMLSchema#"

out = open(OUT, "w", encoding="utf-8")
provinces = {}   # slug -> label


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def lit(s, dt=None, lang=None):
    s = esc(str(s))
    if dt:
        return f'"{s}"^^<{dt}>'
    if lang:
        return f'"{s}"@{lang}'
    return f'"{s}"'


def t(s, p, o):
    out.write(f"<{s}> <{p}> {o} .\n")


def clean(v):
    if v is None:
        return ""
    v = str(v).strip()
    return "" if v.lower() in ("", "null", "none", "-", "n/d", "nd", "sense dades", "desconegut", "desconocido") else v


def strip_accents(s):
    return "".join(c for c in unicodedata.normalize("NFKD", s) if not unicodedata.combining(c))


PROV_VARIANTS = {
    "gerona": ("girona", "Girona"), "girona": ("girona", "Girona"),
    "lerida": ("lleida", "Lleida"), "lleida": ("lleida", "Lleida"),
    "guipuzcoa": ("gipuzkoa", "Gipuzkoa"), "gipuzkoa": ("gipuzkoa", "Gipuzkoa"),
    "vizcaya": ("bizkaia", "Bizkaia"), "bizkaia": ("bizkaia", "Bizkaia"),
    "alava": ("araba", "Araba/Álava"), "araba": ("araba", "Araba/Álava"),
    "araba/alava": ("araba", "Araba/Álava"),
    "nafarroa": ("navarra", "Navarra"), "navarra": ("navarra", "Navarra"),
    "valencia": ("valencia", "València"), "valència": ("valencia", "València"),
    "alicante": ("alicante", "Alacant/Alicante"), "alacant": ("alicante", "Alacant/Alicante"),
    "castellon": ("castellon", "Castelló/Castellón"), "castello": ("castellon", "Castelló/Castellón"),
    "la coruna": ("a_coruna", "A Coruña"), "a coruna": ("a_coruna", "A Coruña"),
    "orense": ("ourense", "Ourense"), "ourense": ("ourense", "Ourense"),
}


def province_nodes(name):
    """Return a list of province IRIs. Splits compound values ('A - B', 'A / B')
    into separate provinces and resolves bilingual single-province forms ('Alicante/Alacant')."""
    raw = clean(name)
    if not raw:
        return []
    nodes = []
    for part in re.split(r"\s+[-/]\s+", raw):   # multi-province separators (spaced - or /)
        part = part.strip()
        if not part:
            continue
        cands = [part] + ([p.strip() for p in part.split("/")] if "/" in part else [])
        resolved = None
        for c in cands:                          # try whole then bilingual halves
            k = strip_accents(c).lower().strip()
            if k in PROV_VARIANTS:
                resolved = PROV_VARIANTS[k]
                break
        if resolved:
            slug, label = resolved
        else:
            k = strip_accents(part).lower().strip()
            slug = re.sub(r"[^a-z0-9]+", "_", k).strip("_")
            label = part
        if slug:
            provinces.setdefault(slug, label)
            nodes.append(f"{B}provincia/{slug}")
    return list(dict.fromkeys(nodes))            # dedupe, keep order


def linkp(s, pred, raw):
    for pv in province_nodes(raw):
        t(s, pred, f"<{pv}>")


def slugid(s):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", str(s)).strip("_")


SEX = {"home": "male", "dona": "female", "h": "male", "d": "female",
       "hombre": "male", "mujer": "female", "v": "male", "m": "female"}


def is_yes(v):
    return clean(v).lower() in ("si", "sí", "s", "yes", "true", "1", "cert")


def reader(path, delim=",", enc="utf-8-sig"):
    f = open(path, encoding=enc, newline="")
    return csv.DictReader(f, delimiter=delim)


# ---------------------------------------------------------------- persons
def person(src, pid, name):
    s = f"{B}persona/{src}/{slugid(pid)}"
    t(s, RDF + "type", f"<{MC}Victim>")
    if name:
        t(s, RDFS + "label", lit(name, lang="es"))
    t(s, DCT + "source", lit(src))
    return s


def do_cat_reparacio():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "cat_reparacio.csv"))):
        pid = clean(r.get("Codi")) or str(i)
        name = clean(r.get("Cognoms nom")) or (clean(r.get("Cognoms")) + " " + clean(r.get("Nom"))).strip()
        s = person("cat-reparacio", pid, name)
        sx = SEX.get(clean(r.get("Sexe")).lower())
        if sx: t(s, MC + "sex", lit(sx))
        age = clean(r.get("Edat"))
        if age.isdigit(): t(s, MC + "age", lit(age, dt=XSD + "integer"))
        mb = clean(r.get("Municipi naixement"))
        if mb: t(s, MC + "bornInMunicipality", lit(mb, lang="ca"))
        linkp(s, MC + "bornInProvince", r.get("Província naixement"))
        mr = clean(r.get("Municipi residència"))
        if mr: t(s, MC + "residedInMunicipality", lit(mr, lang="ca"))
        linkp(s, MC + "residedInProvince", r.get("Província residència"))
        proc = clean(r.get("Tipus procediment 1"))
        if proc: t(s, MC + "procedure", lit(proc, lang="ca"))
        pena = clean(r.get("Pena"))
        if pena: t(s, MC + "sentence", lit(pena, lang="ca"))
        yr = clean(r.get("Any inicial"))
        if yr.isdigit(): t(s, MC + "year", lit(yr, dt=XSD + "gYear"))
        nc = clean(r.get("Num causa"))
        if nc: t(s, MC + "caseNumber", lit(nc))
        if is_yes(r.get("Afusellades")) or "afusell" in pena.lower():
            t(s, MC + "executed", lit("true", dt=XSD + "boolean"))
        n += 1
    return n


def do_cat_desaparecidos():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "cat_desaparecidos.csv"))):
        pid = clean(r.get("Id Afectat")) or str(i)
        s = person("cat-desapareguts", pid, clean(r.get("Nom Desaparegut")))
        sx = SEX.get(clean(r.get("Sexe")).lower())
        if sx: t(s, MC + "sex", lit(sx))
        mb = clean(r.get("Municipi naixement"))
        if mb: t(s, MC + "bornInMunicipality", lit(mb, lang="ca"))
        linkp(s, MC + "bornInProvince", r.get("Província naixement"))
        mr = clean(r.get("Municipi habitual"))
        if mr: t(s, MC + "residedInMunicipality", lit(mr, lang="ca"))
        linkp(s, MC + "residedInProvince", r.get("Província habitual"))
        prof = clean(r.get("Professió"))
        if prof: t(s, MC + "profession", lit(prof, lang="ca"))
        unit = clean(r.get("Unitat militar"))
        if unit: t(s, MC + "militaryUnit", lit(unit, lang="ca"))
        army = clean(r.get("Exèrcit"))
        if army: t(s, MC + "army", lit(army, lang="ca"))
        dd = clean(r.get("Data desaparició")) or clean(r.get("Data afusellament"))
        if dd: t(s, MC + "deathDate", lit(dd))
        place = clean(r.get("Lloc afusellament")) or clean(r.get("Indret desaparició"))
        if place: t(s, MC + "deathPlace", lit(place, lang="ca"))
        if is_yes(r.get("És afusellat")):
            t(s, MC + "executed", lit("true", dt=XSD + "boolean"))
        n += 1
    return n


def do_euskadi():
    data = json.load(open(os.path.join(SRC, "euskadi_victimas.json"), encoding="utf-8"))
    n = 0
    for i, r in enumerate(data):
        name = (clean(r.get("Nombre")) + " " + clean(r.get("Apellidos"))).strip()
        s = person("euskadi", i, name)
        mr = clean(r.get("Municipiodomicilio"))
        if mr: t(s, MC + "residedInMunicipality", lit(mr, lang="es"))
        linkp(s, MC + "residedInProvince", r.get("Provinciadomicilio"))
        cause = clean(r.get("Causamuerte"))
        if cause: t(s, MC + "cause", lit(cause, lang="es"))
        dd = clean(r.get("Fechafallecimiento"))
        if dd: t(s, MC + "deathDate", lit(dd))
        dp = clean(r.get("Lugarfallecimiento"))
        if dp: t(s, MC + "deathPlace", lit(dp, lang="es"))
        linkp(s, MC + "deathProvince", r.get("Provinciafallecimiento"))
        bur = clean(r.get("Lugarinhumacion"))
        if bur: t(s, MC + "burialPlace", lit(bur, lang="es"))
        if "ejecutad" in cause.lower():
            t(s, MC + "executed", lit("true", dt=XSD + "boolean"))
        n += 1
    return n


# ---------------------------------------------------------------- graves
def grave(src, gid, title):
    s = f"{B}fosa/{src}/{slugid(gid)}"
    t(s, RDF + "type", f"<{MC}MassGrave>")
    if title:
        t(s, RDFS + "label", lit(title, lang="es"))
    t(s, DCT + "source", lit(src))
    return s


def add_coords(s, lat, lon):
    try:
        la, lo = float(lat), float(lon)
    except (TypeError, ValueError):
        return
    if 35 <= la <= 44 and -10 <= lo <= 5:
        t(s, GEO + "lat", lit(f"{la}", dt=XSD + "decimal"))
        t(s, GEO + "long", lit(f"{lo}", dt=XSD + "decimal"))
        t(s, GSP + "asWKT", f'"POINT({lo} {la})"^^<{GSP}wktLiteral>')


def do_cat_fosas():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "cat_fosas.csv"))):
        s = grave("cat", i, clean(r.get("Títol")))
        if clean(r.get("Id")): t(s, MC + "sourceId", lit(clean(r.get("Id"))))
        muni = clean(r.get("Municipi"))
        if muni: t(s, MC + "municipality", lit(muni, lang="ca"))
        linkp(s, MC + "province", r.get("Província"))
        cat = clean(r.get("Categoria de fosses"))
        if cat: t(s, MC + "category", lit(cat, lang="ca"))
        cons = clean(r.get("Conservació"))
        if cons: t(s, MC + "status", lit(cons, lang="ca"))
        typ = clean(r.get("Tipologia inhumats"))
        if typ: t(s, MC + "buriedType", lit(typ, lang="ca"))
        mida = clean(r.get("Mida"))
        if mida.isdigit(): t(s, MC + "victimCount", lit(mida, dt=XSD + "integer"))
        fitxa = clean(r.get("Fitxa"))
        if fitxa.startswith("http"): t(s, FOAF + "page", f"<{fitxa}>")
        add_coords(s, r.get("Latitud"), r.get("Longitud"))
        n += 1
    return n


def do_and_fosas():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "and_fosas.csv"))):
        s = grave("and", i, clean(r.get("Titulo")))
        muni = clean(r.get("Municipio"))
        if muni: t(s, MC + "municipality", lit(muni, lang="es"))
        linkp(s, MC + "province", r.get("Provincia"))
        car = clean(r.get("Caracter"))
        if car: t(s, MC + "category", lit(car, lang="es"))
        vic = clean(r.get("Victimas"))
        if vic.isdigit(): t(s, MC + "victimCount", lit(vic, dt=XSD + "integer"))
        rel = clean(r.get("RelatoHistorico"))
        if rel: t(s, MC + "narrative", lit(rel[:2000], lang="es"))
        fch = clean(r.get("Fecha"))
        if fch: t(s, MC + "date", lit(fch))
        url = clean(r.get("URL"))
        if url.startswith("http"): t(s, FOAF + "page", f"<{url}>")
        n += 1
    return n


def do_cyl_fosas():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "cyl_fosas.csv"), delim=";")):
        s = grave("cyl", i, clean(r.get("LOCALIDAD")) or clean(r.get("MUNICIPIO")))
        if clean(r.get("Nº")): t(s, MC + "sourceId", lit(clean(r.get("Nº"))))
        muni = clean(r.get("MUNICIPIO"))
        if muni: t(s, MC + "municipality", lit(muni, lang="es"))
        linkp(s, MC + "province", r.get("PROVINCIA"))
        vic = clean(r.get("Nº DE VÍCTIMAS"))
        if vic.isdigit(): t(s, MC + "victimCount", lit(vic, dt=XSD + "integer"))
        est = clean(r.get("ESTADO"))
        if est: t(s, MC + "status", lit(est, lang="es"))
        side = clean(r.get("BANDO REPRESOR"))
        if side: t(s, MC + "repressorSide", lit(side, lang="es"))
        ex = clean(r.get("Nº DE CUERPOS EXHUMADOS"))
        if ex.isdigit(): t(s, MC + "exhumedCount", lit(ex, dt=XSD + "integer"))
        dt_ = clean(r.get("FECHA DE LA EJECUCIÓN"))
        if dt_: t(s, MC + "date", lit(dt_))
        n += 1
    return n


def do_val_fosas():
    n = 0
    for i, r in enumerate(reader(os.path.join(SRC, "val_fosas.csv"))):
        s = grave("val", i, clean(r.get("name")))
        if clean(r.get("id")): t(s, MC + "sourceId", lit(clean(r.get("id"))))
        muni = clean(r.get("nom_mun_ca")) or clean(r.get("nom_mun_va"))
        if muni: t(s, MC + "municipality", lit(muni, lang="ca"))
        linkp(s, MC + "province", r.get("provincia_ca") or r.get("provincia_va"))
        typ = clean(r.get("tipo_ca"))
        if typ: t(s, MC + "category", lit(typ, lang="ca"))
        est = clean(r.get("estado_ca"))
        if est: t(s, MC + "status", lit(est, lang="ca"))
        pdf = clean(r.get("pdf"))
        if pdf.startswith("http"): t(s, FOAF + "page", f"<{pdf}>")
        n += 1
    return n


def main():
    counts = {}
    counts["cat_reparacio (persons)"] = do_cat_reparacio()
    counts["cat_desaparecidos (persons)"] = do_cat_desaparecidos()
    counts["euskadi_victimas (persons)"] = do_euskadi()
    counts["cat_fosas (graves)"] = do_cat_fosas()
    counts["and_fosas (graves)"] = do_and_fosas()
    counts["cyl_fosas (graves)"] = do_cyl_fosas()
    counts["val_fosas (graves)"] = do_val_fosas()
    for slug, label in provinces.items():
        s = f"{B}provincia/{slug}"
        t(s, RDF + "type", f"<{MC}Province>")
        t(s, RDFS + "label", lit(label, lang="es"))
    out.close()
    total_p = sum(v for k, v in counts.items() if "persons" in k)
    total_g = sum(v for k, v in counts.items() if "graves" in k)
    print("=== per-source ===")
    for k, v in counts.items():
        print(f"  {v:7d}  {k}")
    print(f"  {len(provinces):7d}  provinces")
    print(f"TOTAL: {total_p} persons + {total_g} graves -> {OUT}")


if __name__ == "__main__":
    main()
