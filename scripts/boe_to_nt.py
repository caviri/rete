#!/usr/bin/env python3
"""Build an ELI knowledge graph of Spanish consolidated legislation (the whole
`legislacion-consolidada` corpus of the Boletin Oficial del Estado) as
N-Triples, from the BOE Open-Data API.

Source (official open-data API, no scraping):
  https://www.boe.es/datosabiertos/api/legislacion-consolidada
Reuse: Ley 37/2007 + BOE standard licence (Resolucion 27-jun-2024) -- derived
works permitted; attribution "Basado en datos de la Agencia Estatal Boletin
Oficial del Estado". See https://www.boe.es/informacion/aviso_legal/#reutilizacion

Two API reads per norm are combined:
  * the paginated enumeration -> node metadata (title, rango, dates, author,
    ELI URL, HTML consolidated version)
  * /id/{id}/analisis          -> subjects (materias) + the norm-to-norm
    relationship graph (DEROGA/MODIFICA/CITA/...), the interesting part

Modelling (ELI-aligned so it federates with EUR-Lex and other ELI publishers):
  norm IRI = its real ELI work IRI  https://www.boe.es/eli/es/l/2015/10/01/40
     a  eli:LegalResource , bx:<Rango>            (bx:Ley subClassOf eli:LegalResource)
     eli:id_local / eli:title / rdfs:label
     eli:date_document / eli:date_publication / eli:first_date_entry_in_force  (xsd:date)
     eli:number  eli:type_document->rango  eli:passed_by->org  eli:is_about->materia
     eli:jurisdiction->ambito  bx:consolidationStatus->estado  bx:htmlVersion
  edges: eli:repeals/amends/corrects/cites/transposes/based_on/consolidates/
         changes/related_to  +  bx:publishes/approves/authorizes/ratifies/
         challenges/declares/interprets/accepts/accedesTo
  vocabularies: rango / materia / ambito / estado  -> skos:Concept schemes;
                departamento -> org:Organization.
Referenced norms outside the consolidated set become lightweight
eli:LegalResource nodes (id + best-effort label) so no edge dangles unlabelled.

Usage:  python scripts/boe_to_nt.py [harvest|emit|all]   (default: all)
  harvest  fetch enumeration + aux vocab + every /analisis into data/boe/cache/
  emit     turn the cache into data/boe/boe.nt + data/boe/boe_ont.ttl
Both steps are resumable: cached responses are never re-fetched.
"""
import json
import os
import re
import sys
import time
import unicodedata
import urllib.request
import urllib.error
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "data", "boe")
CACHE = os.path.join(DATA, "cache")
ANDIR = os.path.join(CACHE, "analisis")
NT = os.path.join(DATA, "boe.nt")
ONT = os.path.join(DATA, "boe_ont.ttl")

API = "https://www.boe.es/datosabiertos/api"
UA = "rete-boe/1.0 (graphplaza; open-data reuse; contact carlosvivarrios@gmail.com)"
WORKERS = 10

ELI = "http://data.europa.eu/eli/ontology#"
BX = "https://graphplaza.com/ns/boe#"        # our extension vocabulary (neutral ns)
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT = "http://purl.org/dc/terms/"
ORG = "http://www.w3.org/ns/org#"
DATE = XSD + "date"

# ---- BOE root IRIs (real, dereferenceable BOE identifiers) ----
ELIBASE = "https://www.boe.es/eli"
DOCBASE = "https://www.boe.es/buscar/doc.php?id="   # for norms outside the consolidated set
VOC = "https://www.boe.es/vocab"                     # concept/org IRIs rooted at the source

# ---- helpers (house style, cf. scripts/peirce_to_nt.py) ----
_ctrl = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")


def clean(s):
    return re.sub(r"\s+", " ", _ctrl.sub(" ", s or "")).strip()


def esc(s):
    s = _ctrl.sub(" ", str(s))
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def ci(u):
    u = re.sub(r"\s+", "", str(u))
    return u if u and "<" not in u and ">" not in u and '"' not in u else None


def camel(s):
    s = unicodedata.normalize("NFKD", s or "").encode("ascii", "ignore").decode()
    parts = re.split(r"[^A-Za-z0-9]+", s)
    return "".join(p[:1].upper() + p[1:] for p in parts if p) or "Norma"


def isodate(yyyymmdd):
    s = re.sub(r"\D", "", str(yyyymmdd or ""))
    if len(s) >= 8 and s[:8].isdigit() and s[4:6] != "00" and s[6:8] != "00":
        return f"{s[:4]}-{s[4:6]}-{s[6:8]}"
    return None


def aslist(x):
    if x is None:
        return []
    return x if isinstance(x, list) else [x]


class W:
    def __init__(s, fh):
        s.fh, s.n = fh, 0

    def iri(s, a, p, b):
        a, b = ci(a), ci(b)
        if a and b:
            s.fh.write(f"<{a}> <{p}> <{b}> .\n"); s.n += 1

    def lit(s, a, p, o, dt=None):
        a = ci(a)
        o = clean(o) if isinstance(o, str) else o
        if not a or o is None or o == "":
            return
        o = esc(o)
        s.fh.write(f'<{a}> <{p}> "{o}"^^<{dt}> .\n' if dt else f'<{a}> <{p}> "{o}" .\n')
        s.n += 1


# ---------------------------------------------------------------- fetching
def _get(url, tries=5):
    req = urllib.request.Request(url, headers={"Accept": "application/json", "User-Agent": UA})
    for k in range(tries):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.loads(r.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504) and k < tries - 1:
                time.sleep(2 * (k + 1)); continue
            raise
        except Exception:
            if k < tries - 1:
                time.sleep(2 * (k + 1)); continue
            raise


def harvest():
    os.makedirs(ANDIR, exist_ok=True)
    # 1) enumeration (two pages: the API caps a page at 10k rows)
    enum_f = os.path.join(CACHE, "enum.json")
    if not os.path.exists(enum_f):
        rows = []
        for off in (0, 10000):
            d = _get(f"{API}/legislacion-consolidada?limit=-1&offset={off}")
            rows += d.get("data") or []
            time.sleep(0.5)
        json.dump(rows, open(enum_f, "w", encoding="utf-8"), ensure_ascii=False)
        print(f"enumeration: {len(rows)} norms cached")
    rows = json.load(open(enum_f, encoding="utf-8"))
    # 2) auxiliary controlled vocabularies
    for v in ("rangos", "materias", "departamentos", "ambitos",
              "estados-consolidacion", "relaciones-anteriores", "relaciones-posteriores"):
        f = os.path.join(CACHE, f"aux_{v}.json")
        if not os.path.exists(f):
            json.dump(_get(f"{API}/datos-auxiliares/{v}").get("data"),
                      open(f, "w", encoding="utf-8"), ensure_ascii=False)
    # 3) per-norm /analisis (resumable, concurrent)
    ids = [r["identificador"] for r in rows if r.get("identificador")]
    todo = [i for i in ids if not os.path.exists(os.path.join(ANDIR, i + ".json"))]
    print(f"/analisis: {len(ids)-len(todo)} cached, {len(todo)} to fetch")
    done = [0]

    def one(i):
        try:
            d = _get(f"{API}/legislacion-consolidada/id/{i}/analisis")
            json.dump(d.get("data"), open(os.path.join(ANDIR, i + ".json"), "w",
                      encoding="utf-8"), ensure_ascii=False)
        except Exception as e:
            json.dump({"__err__": str(e)}, open(os.path.join(ANDIR, i + ".json"), "w",
                      encoding="utf-8"))
        done[0] += 1
        if done[0] % 500 == 0:
            print(f"  ...{done[0]}/{len(todo)}", flush=True)

    if todo:
        with ThreadPoolExecutor(WORKERS) as ex:
            list(ex.map(one, todo))
    print("harvest complete")


# ---------------------------------------------------------------- relation map
# BOE relation code -> predicate (normalised to actor -> patient direction).
# ELI standard properties where faithful, else a bx: extension.
def P(local, ns=ELI):
    return ns + local


REL = {
    "201": P("corrects"), "202": P("corrects"), "203": P("corrects"), "204": P("corrects"),
    "210": P("repeals"), "211": P("repeals"), "212": P("repeals"), "213": P("repeals"),
    "214": P("repeals"), "215": P("repeals"), "216": P("repeals"), "217": P("repeals"),
    "220": P("repeals"), "221": P("repeals"), "230": P("repeals"), "235": P("repeals"),
    "231": P("changes"),
    "245": P("amends"), "247": P("amends"),
    "270": P("amends"), "271": P("amends"), "272": P("amends"),
    "404": P("amends"), "406": P("amends"), "407": P("amends"), "408": P("amends"),
    "300": P("publishes", BX), "301": P("publishes", BX), "303": P("publishes", BX),
    "330": P("cites"),
    "331": P("related_to"), "540": P("related_to"), "693": P("related_to"),
    "400": P("accepts", BX),
    "401": P("changes"),
    "402": P("interprets", BX),
    "420": P("approves", BX),
    "421": P("authorizes", BX),
    "422": P("ratifies", BX),
    "426": P("transposes"), "427": P("transposes"),
    "430": P("accedesTo", BX),
    "440": P("based_on"), "490": P("based_on"),
    "470": P("declares", BX), "480": P("declares", BX),
    "520": P("challenges", BX), "530": P("challenges", BX),
    "552": P("challenges", BX), "694": P("challenges", BX),
}
REL_DEFAULT = P("related_to")

# which bx: predicates exist, and the eli: superproperty they refine (for TBox)
BX_PROPS = {
    "publishes": "related_to", "accepts": "related_to", "interprets": "related_to",
    "approves": "related_to", "authorizes": "related_to", "ratifies": "related_to",
    "accedesTo": "related_to", "declares": "related_to", "challenges": "related_to",
}
ELI_PROP_LABEL = {
    "repeals": ("deroga", "repeals"), "amends": ("modifica", "amends"),
    "corrects": ("corrige", "corrects"), "cites": ("cita", "cites"),
    "transposes": ("transpone", "transposes"), "based_on": ("dictada en virtud de", "based on"),
    "consolidates": ("consolida", "consolidates"), "changes": ("altera", "changes"),
    "related_to": ("en relacion con", "related to"),
}
BX_PROP_LABEL = {
    "publishes": ("publica", "publishes"), "accepts": ("acepta", "accepts"),
    "interprets": ("interpreta", "interprets"), "approves": ("aprueba", "approves"),
    "authorizes": ("autoriza", "authorizes"), "ratifies": ("ratifica", "ratifies"),
    "accedesTo": ("se adhiere a", "accedes to"), "declares": ("declara", "declares"),
    "challenges": ("recurre / cuestiona", "challenges"),
}


# ---------------------------------------------------------------- emit
def emit():
    rows = json.load(open(os.path.join(CACHE, "enum.json"), encoding="utf-8"))
    aux = {v: json.load(open(os.path.join(CACHE, f"aux_{v}.json"), encoding="utf-8"))
           for v in ("rangos", "materias", "departamentos", "ambitos", "estados-consolidacion")}

    # id (BOE-A-...) -> ELI work IRI, for resolving edge targets in the corpus
    id2eli, rango_by_id = {}, {}
    for r in rows:
        i = r.get("identificador")
        if i and r.get("url_eli"):
            id2eli[i] = r["url_eli"]
        rango_by_id[i] = (r.get("rango") or {}).get("codigo")

    used_rango, used_mat, used_org, used_amb, used_est = set(), set(), set(), set(), set()
    used_eli_props, used_bx_props, used_classes = set(), set(), set()
    ext_norms = {}   # out-of-corpus target id -> best label

    def org_iri(code): return f"{VOC}/departamento/{code}"
    def rango_iri(code): return f"{VOC}/rango/{code}"
    def mat_iri(code): return f"{VOC}/materia/{code}"
    def amb_iri(code): return f"{VOC}/ambito/{code}"
    def est_iri(code): return f"{VOC}/estado/{code}"

    def target_iri(idn, label):
        if idn in id2eli:
            return id2eli[idn]
        iri = DOCBASE + idn
        # keep the shortest sensible label (references often prefix article lists)
        lab = clean(label)
        m = re.search(r"((?:Ley Org[aá]nica|Ley|Real Decreto(?:-ley| Legislativo)?|"
                      r"Decreto(?:-ley| Legislativo)?|Orden|Reglamento|Resoluci[oó]n|"
                      r"Circular|Instrucci[oó]n|Constituci[oó]n)[^,;]*(?:,\s*de[^,;]*)?)", lab)
        lab = clean(m.group(1)) if m else lab
        if idn not in ext_norms or (lab and len(lab) < len(ext_norms[idn])):
            ext_norms[idn] = lab or idn
        return iri

    fh = open(NT, "w", encoding="utf-8", newline="\n")
    w = W(fh)

    for r in rows:
        i = r.get("identificador")
        s = r.get("url_eli") or (DOCBASE + i if i else None)
        if not s:
            continue
        w.iri(s, RDF + "type", ELI + "LegalResource")
        rg = r.get("rango") or {}
        if rg.get("texto"):
            cls = BX + camel(rg["texto"]); used_classes.add(rg["texto"])
            w.iri(s, RDF + "type", cls)
        if rg.get("codigo"):
            used_rango.add(rg["codigo"])
            w.iri(s, ELI + "type_document", rango_iri(rg["codigo"]))
        if i:
            w.lit(s, ELI + "id_local", i, XSD + "string")
        t = clean(r.get("titulo"))
        if t:
            w.lit(s, ELI + "title", t); w.lit(s, RDFS + "label", t); w.lit(s, DCT + "title", t)
        for pr, key in ((ELI + "date_document", "fecha_disposicion"),
                        (ELI + "date_publication", "fecha_publicacion"),
                        (ELI + "first_date_entry_in_force", "fecha_vigencia")):
            d = isodate(r.get(key))
            if d:
                w.lit(s, pr, d, DATE)
        if r.get("numero_oficial"):
            w.lit(s, ELI + "number", r["numero_oficial"], XSD + "string")
        dep = r.get("departamento") or {}
        if dep.get("codigo"):
            used_org.add(dep["codigo"]); w.iri(s, ELI + "passed_by", org_iri(dep["codigo"]))
        amb = r.get("ambito") or {}
        if amb.get("codigo"):
            used_amb.add(amb["codigo"]); w.iri(s, ELI + "jurisdiction", amb_iri(amb["codigo"]))
        est = r.get("estado_consolidacion") or {}
        if est.get("codigo"):
            used_est.add(est["codigo"]); w.iri(s, BX + "consolidationStatus", est_iri(est["codigo"]))
        if r.get("diario_numero"):
            w.lit(s, BX + "gazetteNumber", r["diario_numero"], XSD + "string")
        va = r.get("vigencia_agotada")
        if va in ("S", "N"):
            w.lit(s, BX + "spentValidity", "true" if va == "S" else "false", XSD + "boolean")
        if r.get("url_html_consolidada"):
            w.iri(s, BX + "htmlVersion", r["url_html_consolidada"])
        if r.get("url_eli"):
            w.iri(s, BX + "eliUri", r["url_eli"])
        if r.get("fecha_actualizacion"):
            fa = r["fecha_actualizacion"][:8]
            d = isodate(fa)
            if d:
                w.lit(s, DCT + "modified", d, DATE)

        # --- analisis: subjects + relationship edges ---
        af = os.path.join(ANDIR, (i or "") + ".json")
        if not i or not os.path.exists(af):
            continue
        try:
            data = json.load(open(af, encoding="utf-8"))
        except Exception:
            continue
        if isinstance(data, dict) and data.get("__err__"):
            continue
        for blk in aslist(data):
            if not isinstance(blk, dict):
                continue
            for mw in aslist(blk.get("materias")):
                m = mw.get("materia") if isinstance(mw, dict) else None
                for mm in aslist(m):
                    if isinstance(mm, dict) and mm.get("codigo"):
                        used_mat.add(mm["codigo"])
                        w.iri(s, ELI + "is_about", mat_iri(mm["codigo"]))
            refs = blk.get("referencias") or {}
            for sec, sing in (("anteriores", "anterior"), ("posteriores", "posterior")):
                for wrap in aslist(refs.get(sec)):
                    items = aslist(wrap.get(sing)) if isinstance(wrap, dict) else []
                    for it in items:
                        if not isinstance(it, dict) or not it.get("id_norma"):
                            continue
                        tgt_id = it["id_norma"]
                        rel = it.get("relacion") or {}
                        pred = REL.get(str(rel.get("codigo")), REL_DEFAULT)
                        tgt = target_iri(tgt_id, it.get("texto") or "")
                        # anteriores: this -> target ; posteriores: target -> this
                        a, b = (s, tgt) if sec == "anteriores" else (tgt, s)
                        w.iri(a, pred, b)
                        loc = pred.rsplit("#", 1)[1] if "#" in pred else pred
                        (used_bx_props if pred.startswith(BX) else used_eli_props).add(loc)

    # ---- controlled vocabularies (only concepts actually referenced) ----
    ramap = {str(k): v for k, v in (aux["rangos"] or {}).items()}
    mamap = {str(k): v for k, v in (aux["materias"] or {}).items()}
    demap = {str(k): v for k, v in (aux["departamentos"] or {}).items()}
    ammap = {str(k): v for k, v in (aux["ambitos"] or {}).items()}
    esmap = {str(k): v for k, v in (aux["estados-consolidacion"] or {}).items()}

    def scheme(iri, code_iri_fn, codes, label_map, scheme_iri, cls=SKOS + "Concept"):
        for c in sorted(codes):
            u = code_iri_fn(c)
            w.iri(u, RDF + "type", cls)
            lab = clean(label_map.get(str(c), str(c)))
            w.lit(u, SKOS + "prefLabel", lab); w.lit(u, RDFS + "label", lab)
            if scheme_iri:
                w.iri(u, SKOS + "inScheme", scheme_iri)

    scheme(None, rango_iri, used_rango, ramap, f"{VOC}/rangos")
    scheme(None, mat_iri, used_mat, mamap, f"{VOC}/materias")
    scheme(None, amb_iri, used_amb, ammap, f"{VOC}/ambitos")
    scheme(None, est_iri, used_est, esmap, f"{VOC}/estados")
    for c in sorted(used_org):
        u = org_iri(c)
        w.iri(u, RDF + "type", ORG + "Organization")
        lab = clean(demap.get(str(c), str(c)))
        w.lit(u, RDFS + "label", lab); w.lit(u, SKOS + "prefLabel", lab)

    # ---- out-of-corpus referenced norms (lightweight nodes) ----
    for idn, lab in ext_norms.items():
        u = DOCBASE + idn
        w.iri(u, RDF + "type", ELI + "LegalResource")
        w.lit(u, ELI + "id_local", idn, XSD + "string")
        w.lit(u, RDFS + "label", lab); w.lit(u, ELI + "title", lab)

    fh.close()
    write_ontology(used_classes, used_eli_props, used_bx_props, ramap)
    print(f"wrote {NT}  ({w.n} triples)")
    print(f"  in-corpus norms={len(rows)}  external-refs={len(ext_norms)}  "
          f"materias={len(used_mat)}  orgs={len(used_org)}  classes={len(used_classes)}")
    print(f"  edge predicates: eli={sorted(used_eli_props)}  bx={sorted(used_bx_props)}")


def write_ontology(classes, eli_props, bx_props, ramap):
    L = []
    L.append("@prefix eli:  <%s> ." % ELI)
    L.append("@prefix bx:   <%s> ." % BX)
    L.append("@prefix rdfs: <%s> ." % RDFS)
    L.append("@prefix owl:  <%s> ." % OWL)
    L.append("@prefix skos: <%s> ." % SKOS)
    L.append("@prefix dct:  <%s> ." % DCT)
    L.append("")
    L.append("<%s> a owl:Ontology ;" % BX.rstrip("#"))
    L.append('  rdfs:label "BOE / ELI extension vocabulary"@en ;')
    L.append('  rdfs:comment "Extension terms for the Spanish consolidated-legislation '
             'knowledge graph. Norms use their canonical ELI IRIs and the ELI ontology; '
             'these bx: terms cover BOE-specific relations and attributes not in ELI. '
             'Derived from BOE open data (Ley 37/2007)."@en .')
    L.append("")
    L.append("eli:LegalResource a owl:Class ; rdfs:label \"Recurso legal (ELI)\"@es .")
    for txt in sorted(classes):
        cls = camel(txt)
        L.append('bx:%s a owl:Class ; rdfs:subClassOf eli:LegalResource ; '
                 'rdfs:label "%s"@es .' % (cls, esc(txt)))
    L.append("")
    for p in sorted(eli_props):
        es, en = ELI_PROP_LABEL.get(p, (p, p))
        L.append('eli:%s a owl:ObjectProperty ; rdfs:label "%s"@es , "%s"@en .' % (p, es, en))
    for p in sorted(bx_props):
        es, en = BX_PROP_LABEL.get(p, (p, p))
        sup = BX_PROPS.get(p)
        sub = " ; rdfs:subPropertyOf eli:%s" % sup if sup else ""
        L.append('bx:%s a owl:ObjectProperty%s ; rdfs:label "%s"@es , "%s"@en .' % (p, sub, es, en))
    L.append("")
    L.append('bx:consolidationStatus a owl:ObjectProperty ; rdfs:label "estado de consolidacion"@es .')
    L.append('bx:htmlVersion a owl:ObjectProperty ; rdfs:label "version consolidada (HTML)"@es .')
    L.append('bx:eliUri a owl:ObjectProperty ; rdfs:label "ELI URI"@es .')
    L.append('bx:gazetteNumber a owl:DatatypeProperty ; rdfs:label "numero de diario"@es .')
    L.append('bx:spentValidity a owl:DatatypeProperty ; rdfs:label "vigencia agotada"@es .')
    open(ONT, "w", encoding="utf-8", newline="\n").write("\n".join(L) + "\n")
    print(f"wrote {ONT}")


if __name__ == "__main__":
    os.makedirs(CACHE, exist_ok=True)
    mode = sys.argv[1] if len(sys.argv) > 1 else "all"
    if mode in ("harvest", "all"):
        harvest()
    if mode in ("emit", "all"):
        emit()
