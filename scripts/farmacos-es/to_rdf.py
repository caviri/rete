#!/usr/bin/env python3
"""Model the CIMA (AEMPS) harvest as a medical knowledge graph in N-Triples.

Input : data/farmacos-es/raw/  (see scripts/farmacos-es/harvest.py)
Output: data/farmacos-es/farmacos-es.nt      (instances)
        data/farmacos-es/farmacos-es_ont.ttl  (ontology / TBox)

Ontology reuse & cross-links to standard medical vocabularies
-------------------------------------------------------------
  schema.org   Medicamento subClassOf schema:Drug, Laboratorio subClassOf
               schema:MedicalOrganization; schema:activeIngredient/manufacturer/url.
  SKOS         ATC, principios activos, excipientes, vias, formas, gravedad de
               interaccion, frecuencia de efectos -> skos:Concept schemes.
  SNOMED CT    vtm (moiety) + cod_dcsa/cod_dcp/cod_dcpf (Spanish drug extension)
               -> http://snomed.info/id/{sctid}.
  WHO ATC      ConceptoATC carries the ATC code (skos:notation) in the real ATC
               hierarchy (skos:broader) + rdfs:seeAlso WHOCC + skos:exactMatch
               BioPortal ATC.

Structured medical "text" extraction (already coded at source):
  * Drug-drug interactions at ATC level (efecto + recomendacion + derived gravedad)
    from the Nomenclator Prescripcion.xml  -> fmc:Interaccion nodes.
  * Declared excipients (allergen/intolerance info) per presentation.
  * Prospecto/ficha tecnica split into EU QRD template sections, full text kept as
    TEXT_INDEX-able literals typed by section; adverse-effect frequency bands tagged.

Usage:  python scripts/farmacos-es/to_rdf.py [--limit N]
"""
import argparse
import html
import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
RAW = ROOT / "data" / "farmacos-es" / "raw"
NOM = RAW / "nomenclator"
OUT_NT = ROOT / "data" / "farmacos-es" / "farmacos-es.nt"
OUT_ONT = ROOT / "data" / "farmacos-es" / "farmacos-es_ont.ttl"

# ---- namespaces ----
FMC = "https://w3id.org/rete/farmacos#"                 # ontology terms
B = "https://w3id.org/rete/farmacos/"                   # instance base
MED = B + "medicamento/"
PRES = B + "presentacion/"
PA = B + "principio-activo/"
LAB = B + "laboratorio/"
ATC = B + "atc/"
EXC = B + "excipiente/"
VIA = B + "via/"
FF = B + "forma-farmaceutica/"
ING = B + "ingrediente/"
DOC = B + "seccion/"
NOTA = B + "nota/"
INT = B + "interaccion/"
FREQ = B + "frecuencia/"
GRAV = B + "gravedad/"
SNOMED = "http://snomed.info/id/"

SCH = "https://schema.org/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT = "http://purl.org/dc/terms/"

A = RDF + "type"
LABEL = RDFS + "label"
NOTATION = SKOS + "notation"
PREFLABEL = SKOS + "prefLabel"
BROADER = SKOS + "broader"
DATE = XSD + "date"
BOOL = XSD + "boolean"
INT_DT = XSD + "integer"

# ---- helpers (house style, cf. scripts/boe_to_nt.py) ----
_ctrl = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")
_tag = re.compile(r"<[^>]+>")


def clean(s):
    return re.sub(r"\s+", " ", _ctrl.sub(" ", s or "")).strip()


def esc(s):
    s = _ctrl.sub(" ", str(s))
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def ci(u):
    u = re.sub(r"\s+", "", str(u))
    return u if u and "<" not in u and ">" not in u and '"' not in u else None


def slug(s):
    return re.sub(r"[^A-Za-z0-9._-]+", "-", str(s).strip()).strip("-")


def html2text(h):
    t = _tag.sub(" ", h or "")
    t = html.unescape(t)
    return re.sub(r"\s+", " ", t).strip()


def epoch_to_date(ms):
    try:
        return (datetime(1970, 1, 1) + timedelta(milliseconds=int(ms))).strftime("%Y-%m-%d")
    except Exception:
        return None


def local(tag):
    return tag.split("}")[-1]


class W:
    def __init__(self, fh):
        self.fh, self.n = fh, 0

    def iri(self, a, p, b):
        a, b = ci(a), ci(b)
        if a and b:
            self.fh.write(f"<{a}> <{p}> <{b}> .\n"); self.n += 1

    def lit(self, a, p, o, dt=None):
        a = ci(a)
        o = clean(o) if (isinstance(o, str) and dt is None) else o
        if not a or o is None or o == "":
            return
        o = esc(o)
        self.fh.write(f'<{a}> <{p}> "{o}"^^<{dt}> .\n' if dt else f'<{a}> <{p}> "{o}" .\n')
        self.n += 1


# ---------------------------------------------------------------- dictionaries
def parse_dict(fname, rectag):
    rows = []
    for _, el in ET.iterparse(NOM / fname, events=("end",)):
        if local(el.tag) == rectag:
            rows.append({local(c.tag): (c.text or "").strip() for c in el})
            el.clear()
    return rows


# ---- ATC hierarchy: parent by ATC level lengths (1,3,4,5,7) ----
def atc_parent(code):
    n = len(code)
    return {3: 1, 4: 3, 5: 4, 7: 5}.get(n) and code[:{3: 1, 4: 3, 5: 4, 7: 5}[n]]


FREQ_TERMS = [
    ("muy-frecuentes", r"muy\s+frecuentes?", "Muy frecuentes (>=1/10)"),
    ("frecuentes", r"(?<!muy\s)(?<!poco\s)frecuentes?", "Frecuentes (1/100 a 1/10)"),
    ("poco-frecuentes", r"poco\s+frecuentes?", "Poco frecuentes (1/1.000 a 1/100)"),
    ("muy-raras", r"muy\s+raras?", "Muy raras (<1/10.000)"),
    ("raras", r"(?<!muy\s)raras?", "Raras (1/10.000 a 1/1.000)"),
    ("no-conocida", r"(?:frecuencia\s+)?no\s+conocida", "Frecuencia no conocida"),
]

# QRD prospecto section (tipo 2) -> class local name
PROSPECTO_SEC = {
    "0": ("Introduccion", "Introducción"),
    "1": ("Indicaciones", "Qué es y para qué se utiliza"),
    "2": ("ContraindicacionesAdvertencias", "Antes de tomar (contraindicaciones, advertencias, interacciones)"),
    "3": ("Posologia", "Cómo tomar (posología)"),
    "4": ("EfectosAdversos", "Posibles efectos adversos"),
    "5": ("Conservacion", "Conservación"),
    "6": ("Composicion", "Contenido del envase e información adicional"),
}


def gravedad(recomendacion):
    r = (recomendacion or "").lower()
    if "contraindicad" in r:
        return "contraindicada", "Asociación contraindicada"
    if "no recomend" in r or "evitar" in r or "no se recomienda" in r or "no debe" in r:
        return "no-recomendada", "Asociación no recomendada"
    if ("precauci" in r or "ajust" in r or "vigil" in r or "monitor" in r
            or "control" in r or "reduc" in r or "separar" in r):
        return "precaucion", "Usar con precaución / ajuste"
    return "informativa", "Interacción informativa"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=None, help="cap medicines (smoke test)")
    args = ap.parse_args()

    print("loading dictionaries ...", flush=True)
    atc = parse_dict("DICCIONARIO_ATC.xml", "atc")
    atc_by_code = {r["codigoatc"]: r for r in atc}
    pa = parse_dict("DICCIONARIO_PRINCIPIOS_ACTIVOS.xml", "principiosactivos")
    pa_by_codigo = {r["codigoprincipioactivo"]: r for r in pa}
    pa_nro_to_codigo = {r["nroprincipioactivo"]: r["codigoprincipioactivo"] for r in pa}
    exc = parse_dict("DICCIONARIO_EXCIPIENTES_DECL_OBLIGATORIA.xml", "excipientes")
    exc_by_code = {r["codigoedo"]: r for r in exc}
    lab = parse_dict("DICCIONARIO_LABORATORIOS.xml", "laboratorios")
    lab_by_code = {r["codigolaboratorio"]: r for r in lab}
    lab_by_name = {re.sub(r"\s+", " ", r["laboratorio"].upper().strip()): r for r in lab}
    print(f"  atc={len(atc)} pa={len(pa)} exc={len(exc)} lab={len(lab)}", flush=True)

    nt = open(OUT_NT, "w", encoding="utf-8", newline="\n")
    w = W(nt)

    emitted_pa, emitted_lab, emitted_exc = set(), set(), set()
    emitted_via, emitted_ff, emitted_snomed = set(), set(), set()

    # ---- ATC concept scheme (full hierarchy) ----
    for r in atc:
        code = r["codigoatc"]
        u = ATC + code
        label = re.sub(r"^[A-Z0-9]+\s*-\s*", "", r["descatc"]).strip()
        w.iri(u, A, FMC + "ConceptoATC")
        w.lit(u, PREFLABEL, label)
        w.lit(u, LABEL, label)
        w.lit(u, NOTATION, code)
        w.iri(u, RDFS + "seeAlso", f"https://www.whocc.no/atc_ddd_index/?code={code}")
        w.iri(u, SKOS + "exactMatch", f"http://purl.bioontology.org/ontology/ATC/{code}")
        p = atc_parent(code)
        if p and p in atc_by_code:
            w.iri(u, BROADER, ATC + p)

    def emit_pa(codigo):
        if not codigo or codigo in emitted_pa:
            return PA + slug(codigo) if codigo else None
        emitted_pa.add(codigo)
        u = PA + slug(codigo)
        r = pa_by_codigo.get(codigo)
        w.iri(u, A, FMC + "PrincipioActivo")
        w.lit(u, NOTATION, codigo)
        if r:
            w.lit(u, PREFLABEL, r["principioactivo"])
            w.lit(u, LABEL, r["principioactivo"])
        return u

    def emit_lab_by_name(name):
        r = lab_by_name.get(re.sub(r"\s+", " ", (name or "").upper().strip()))
        if not r:
            return None
        return emit_lab(r["codigolaboratorio"])

    def emit_lab(code):
        if not code:
            return None
        u = LAB + slug(code)
        if code in emitted_lab:
            return u
        emitted_lab.add(code)
        r = lab_by_code.get(code)
        w.iri(u, A, FMC + "Laboratorio")
        if r:
            w.lit(u, SCH + "name", r["laboratorio"])
            w.lit(u, LABEL, r["laboratorio"])
            if r.get("localidad"):
                w.lit(u, SCH + "addressLocality", r["localidad"])
            if r.get("cif"):
                w.lit(u, FMC + "cif", r["cif"])
        return u

    def emit_exc(code):
        if not code:
            return None
        u = EXC + slug(code)
        if code in emitted_exc:
            return u
        emitted_exc.add(code)
        r = exc_by_code.get(code)
        w.iri(u, A, FMC + "Excipiente")
        w.lit(u, NOTATION, code)
        if r:
            w.lit(u, PREFLABEL, r["edo"])
            w.lit(u, LABEL, r["edo"])
        return u

    def emit_snomed(sctid, label=None, kind=None):
        u = SNOMED + slug(sctid)
        if sctid not in emitted_snomed:
            emitted_snomed.add(sctid)
            w.iri(u, A, SKOS + "Concept")
            w.lit(u, NOTATION, sctid)
            if label:
                w.lit(u, LABEL, label)
            if kind:
                w.lit(u, FMC + "snomedTipo", kind)
        elif label:
            w.lit(u, LABEL, label)
        return u

    def med_iri(nreg):
        return MED + slug(nreg)

    # ---- pass 1: medicines from detail JSONs ----
    print("pass 1: medicines ...", flush=True)
    det_dir = RAW / "medicamentos" / "detalle"
    files = sorted(det_dir.glob("*.json"))
    if args.limit:
        files = files[: args.limit]
    valid_nreg = set()
    n_med = 0
    for f in files:
        try:
            m = json.loads(f.read_bytes())
        except Exception:
            continue
        nreg = str(m.get("nregistro") or "").strip()
        if not nreg:
            continue
        valid_nreg.add(nreg)
        u = med_iri(nreg)
        w.iri(u, A, FMC + "Medicamento")
        w.lit(u, SCH + "name", m.get("nombre"))
        w.lit(u, LABEL, m.get("nombre"))
        w.lit(u, NOTATION, nreg)
        w.lit(u, FMC + "condicionPrescripcion", m.get("cpresc"))
        w.lit(u, SCH + "prescriptionStatus", m.get("cpresc"))
        if m.get("dosis"):
            w.lit(u, FMC + "dosis", m.get("dosis"))
        for flag, prop in [("generico", "esGenerico"), ("huerfano", "esHuerfano"),
                           ("biosimilar", "esBiosimilar"), ("triangulo", "trianguloNegro"),
                           ("conduc", "afectaConduccion"), ("comerc", "comercializado"),
                           ("receta", "requiereReceta"), ("ema", "autorizadoEMA")]:
            if flag in m:
                w.lit(u, FMC + prop, "true" if m[flag] else "false", BOOL)
        d = epoch_to_date((m.get("estado") or {}).get("aut"))
        if d:
            w.lit(u, DCT + "issued", d, DATE)
        # ATC (link every level; deepest is the leaf)
        for a in m.get("atcs", []):
            code = a.get("codigo")
            if code and code in atc_by_code:
                w.iri(u, FMC + "atc", ATC + code)
        # active ingredients (reified with dose)
        for pa_row in m.get("principiosActivos", []):
            codigo = pa_row.get("codigo")
            pau = emit_pa(codigo)
            if not pau:
                continue
            w.iri(u, SCH + "activeIngredient", pau)
            iu = ING + f"{slug(nreg)}-{pa_row.get('orden', 0)}"
            w.iri(u, FMC + "ingrediente", iu)
            w.iri(iu, A, FMC + "Ingrediente")
            w.iri(iu, FMC + "principioActivo", pau)
            w.lit(iu, FMC + "cantidad", pa_row.get("cantidad"))
            w.lit(iu, FMC + "unidad", pa_row.get("unidad"))
            if pa_row.get("orden") is not None:
                w.lit(iu, FMC + "orden", str(pa_row["orden"]), INT_DT)
        # vias
        for v in m.get("viasAdministracion", []):
            vid = str(v.get("id"))
            vu = VIA + slug(vid)
            w.iri(u, FMC + "via", vu)
            if vid not in emitted_via:
                emitted_via.add(vid)
                w.iri(vu, A, FMC + "ViaAdministracion")
                w.lit(vu, PREFLABEL, v.get("nombre"))
                w.lit(vu, LABEL, v.get("nombre"))
        # forma farmaceutica
        ffo = m.get("formaFarmaceutica") or {}
        if ffo.get("id") is not None:
            fid = str(ffo["id"])
            fu = FF + slug(fid)
            w.iri(u, FMC + "formaFarmaceutica", fu)
            if fid not in emitted_ff:
                emitted_ff.add(fid)
                w.iri(fu, A, FMC + "FormaFarmaceutica")
                w.lit(fu, PREFLABEL, ffo.get("nombre"))
                w.lit(fu, LABEL, ffo.get("nombre"))
        # laboratories (by name -> code)
        lt = emit_lab_by_name(m.get("labtitular"))
        if lt:
            w.iri(u, FMC + "laboratorioTitular", lt)
            w.iri(u, SCH + "manufacturer", lt)
        lc = emit_lab_by_name(m.get("labcomercializador"))
        if lc:
            w.iri(u, FMC + "laboratorioComercializador", lc)
        # VTM -> SNOMED
        vtm = m.get("vtm") or {}
        if vtm.get("id"):
            su = emit_snomed(str(vtm["id"]), vtm.get("nombre"), "VTM (moiety terapéutica)")
            w.iri(u, FMC + "vtm", su)
        # presentations (CN)
        for p in m.get("presentaciones", []):
            cn = str(p.get("cn") or "").strip()
            if not cn:
                continue
            pu = PRES + slug(cn)
            w.iri(u, FMC + "presentacion", pu)
            w.iri(pu, A, FMC + "Presentacion")
            w.iri(pu, FMC + "medicamento", u)
            w.lit(pu, SCH + "name", p.get("nombre"))
            w.lit(pu, LABEL, p.get("nombre"))
            w.lit(pu, NOTATION, cn)
            w.lit(pu, FMC + "codigoNacional", cn)
            if p.get("comerc") is not None:
                w.lit(pu, FMC + "comercializado", "true" if p["comerc"] else "false", BOOL)
        # document links + notas flag
        for doc in m.get("docs", []):
            if doc.get("url"):
                prop = "urlFichaTecnicaPdf" if doc.get("tipo") == 1 else "urlProspectoPdf"
                w.iri(u, FMC + prop, doc["url"])
            if doc.get("urlHtml"):
                prop = "urlFichaTecnica" if doc.get("tipo") == 1 else "urlProspecto"
                w.iri(u, FMC + prop, doc["urlHtml"])
                w.iri(u, SCH + "url", doc["urlHtml"])
        n_med += 1
        if n_med % 5000 == 0:
            print(f"  medicines {n_med} | triples {w.n}", flush=True)
    print(f"  medicines total {n_med} | triples {w.n}", flush=True)

    # ---- pass 2: document sections (prospecto + ficha tecnica) ----
    print("pass 2: document sections ...", flush=True)
    for kind, subdir, is_prosp in [("p", "p_secc", True), ("ft", "ft_secc", False)]:
        cls_doc = "SeccionProspecto" if is_prosp else "SeccionFichaTecnica"
        link_prop = "seccionProspecto" if is_prosp else "seccionFichaTecnica"
        n_sec = 0
        for f in (RAW / "docs" / subdir).glob("*.json"):
            nreg = f.stem
            if nreg not in valid_nreg:
                continue
            try:
                secs = json.loads(f.read_bytes())
            except Exception:
                continue
            mu = med_iri(nreg)
            for s in secs:
                secnum = str(s.get("seccion", "")).strip()
                text = html2text(s.get("contenido"))
                if not text:
                    continue
                su = DOC + f"{kind}/{slug(nreg)}/{slug(secnum) or '0'}"
                w.iri(mu, FMC + link_prop, su)
                w.iri(su, A, FMC + cls_doc)
                w.iri(su, A, FMC + "SeccionDocumento")
                w.iri(su, FMC + "medicamento", mu)
                w.lit(su, FMC + "seccionNumero", secnum)
                w.lit(su, FMC + "seccionTitulo", s.get("titulo"))
                w.lit(su, LABEL, s.get("titulo"))
                w.lit(su, FMC + "texto", text)
                w.lit(su, SCH + "text", text)
                # QRD typing for prospecto
                if is_prosp and secnum in PROSPECTO_SEC:
                    cls, _lbl = PROSPECTO_SEC[secnum]
                    w.iri(su, A, FMC + cls)
                    # adverse-effect frequency bands (heuristic, tagged as such)
                    if cls == "EfectosAdversos":
                        low = text.lower()
                        for fid, pat, _flabel in FREQ_TERMS:
                            if re.search(pat, low):
                                w.iri(su, FMC + "mencionaFrecuencia", FREQ + fid)
                n_sec += 1
        print(f"  {subdir}: {n_sec} sections | triples {w.n}", flush=True)

    # ---- pass 3: notas de seguridad ----
    print("pass 3: notas de seguridad ...", flush=True)
    n_nota = 0
    seen_nota = set()
    for f in (RAW / "notas").glob("*.json"):
        nreg = f.stem
        if nreg not in valid_nreg:
            continue
        try:
            notas = json.loads(f.read_bytes())
        except Exception:
            continue
        mu = med_iri(nreg)
        for nt_ in notas:
            ref = nt_.get("referencia") or nt_.get("num") or ""
            nu = NOTA + slug(ref) if ref else NOTA + f"{slug(nreg)}-{n_nota}"
            w.iri(mu, FMC + "notaSeguridad", nu)
            if nu in seen_nota:
                continue
            seen_nota.add(nu)
            w.iri(nu, A, FMC + "NotaSeguridad")
            w.lit(nu, DCT + "title", nt_.get("asunto"))
            w.lit(nu, LABEL, nt_.get("asunto") or ref)
            w.lit(nu, FMC + "referencia", ref)
            d = epoch_to_date(nt_.get("fecha"))
            if d:
                w.lit(nu, DCT + "issued", d, DATE)
            if nt_.get("url"):
                w.iri(nu, FMC + "url", nt_["url"])
        n_nota += 1
    print(f"  notas: {n_nota} files | triples {w.n}", flush=True)

    # ---- pass 4: Prescripcion.xml -> presentation composition, excipients, interactions ----
    print("pass 4: Prescripcion.xml (composition, excipients, interactions) ...", flush=True)
    seen_interaction = set()   # (nreg, atc_target, efecto) dedup across a med's presentations
    n_pres = n_int = 0
    for _, el in ET.iterparse(NOM / "Prescripcion.xml", events=("end",)):
        if local(el.tag) != "prescription":
            continue
        get = {}
        for c in el:
            get[local(c.tag)] = c
        nreg = (get["nro_definitivo"].text or "").strip() if "nro_definitivo" in get else ""
        cn = (get["cod_nacion"].text or "").strip() if "cod_nacion" in get else ""
        if not nreg or nreg not in valid_nreg or not cn:
            el.clear(); continue
        pu = PRES + slug(cn)
        mu = med_iri(nreg)
        # presentation is created in pass 1 only if listed; ensure link + type anyway
        w.iri(pu, A, FMC + "Presentacion")
        w.iri(pu, FMC + "medicamento", mu)
        w.iri(mu, FMC + "presentacion", pu)

        def txt(tag):
            return (get[tag].text or "").strip() if tag in get and get[tag].text else None

        if txt("des_prese"):
            w.lit(pu, SCH + "name", txt("des_prese"))
        # SNOMED CT codes (Spanish drug extension)
        for tag, prop, kind in [("cod_dcsa", "snomedSustancia", "DCSA (sustancia clínica)"),
                                ("cod_dcp", "snomedProducto", "DCP (producto clínico)"),
                                ("cod_dcpf", "snomedProductoForma", "DCPF (producto + forma)")]:
            v = txt(tag)
            if v and v.isdigit():
                su = emit_snomed(v, None, kind)
                w.iri(pu, FMC + prop, su)
        for tag, prop in [("sw_uso_hospitalario", "usoHospitalario"),
                          ("sw_psicotropo", "psicotropo"), ("sw_estupefaciente", "estupefaciente"),
                          ("sw_envase_clinico", "envaseClinico")]:
            v = txt(tag)
            if v is not None:
                w.lit(pu, FMC + prop, "true" if v == "1" else "false", BOOL)
        if txt("laboratorio_titular"):
            lu = emit_lab(txt("laboratorio_titular"))
            if lu:
                w.iri(pu, FMC + "laboratorioTitular", lu)
        # excipientes (allergen/intolerance info)
        for exel in el.iter():
            if local(exel.tag) == "cod_excipiente" and exel.text:
                eu = emit_exc(exel.text.strip())
                if eu:
                    w.iri(pu, FMC + "excipiente", eu)
        # ATC-level interactions -> reified fmc:Interaccion on the medicine
        for ie in el.iter():
            if local(ie.tag) != "interacciones_atc":
                continue
            kids = {local(c.tag): (c.text or "").strip() for c in ie}
            atc_t = kids.get("atc_interaccion")
            efecto = kids.get("efecto_interaccion")
            reco = kids.get("recomendacion_interaccion")
            desc = kids.get("descripcion_atc_interaccion")
            if not atc_t and not desc:
                continue
            key = (nreg, atc_t or desc, (efecto or "")[:60])
            if key in seen_interaction:
                continue
            seen_interaction.add(key)
            iu = INT + f"{slug(nreg)}-{len(seen_interaction)}"
            w.iri(mu, FMC + "interaccion", iu)
            w.iri(iu, A, FMC + "Interaccion")
            w.iri(iu, FMC + "medicamento", mu)
            if atc_t and atc_t in atc_by_code:
                w.iri(iu, FMC + "atcObjetivo", ATC + atc_t)
            if atc_t:
                w.lit(iu, FMC + "codigoAtcObjetivo", atc_t)
            w.lit(iu, FMC + "sustanciaObjetivo", desc)
            w.lit(iu, LABEL, desc or atc_t)
            w.lit(iu, FMC + "efecto", efecto)
            w.lit(iu, FMC + "recomendacion", reco)
            gid, glabel = gravedad(reco)
            w.iri(iu, FMC + "gravedad", GRAV + gid)
            n_int += 1
        n_pres += 1
        if n_pres % 20000 == 0:
            print(f"  presentations {n_pres} | interactions {n_int} | triples {w.n}", flush=True)
        el.clear()
    print(f"  presentations {n_pres} | interactions {n_int} | triples {w.n}", flush=True)

    nt.close()
    print(f"WROTE {OUT_NT}  ({w.n} triples)", flush=True)
    write_ontology()
    print("WROTE ontology", OUT_ONT, flush=True)


def write_ontology():
    ttl = f"""@prefix fmc: <{FMC}> .
@prefix rdfs: <{RDFS}> .
@prefix owl: <{OWL}> .
@prefix skos: <{SKOS}> .
@prefix schema: <{SCH}> .
@prefix dct: <{DCT}> .
@prefix xsd: <{XSD}> .

<{FMC.rstrip('#')}> a owl:Ontology ;
  rdfs:label "Ontología de medicamentos autorizados en España (CIMA/AEMPS)"@es ;
  dct:description "Modelo del catálogo CIMA de la AEMPS: medicamentos, presentaciones, principios activos, laboratorios, clasificación ATC (OMS), conceptos SNOMED CT, excipientes, interacciones a nivel ATC y secciones de prospecto/ficha técnica (plantilla QRD)."@es ;
  dct:source <https://cima.aemps.es/> .

# ---- classes ----
fmc:Medicamento a owl:Class ; rdfs:subClassOf schema:Drug ;
  rdfs:label "Medicamento"@es ; rdfs:comment "Medicamento autorizado (nº de registro CIMA)."@es .
fmc:Presentacion a owl:Class ; rdfs:label "Presentación"@es ;
  rdfs:comment "Formato comercial concreto (código nacional / CN)."@es .
fmc:PrincipioActivo a owl:Class ; rdfs:subClassOf skos:Concept ; rdfs:label "Principio activo"@es .
fmc:Ingrediente a owl:Class ; rdfs:label "Ingrediente"@es ;
  rdfs:comment "Principio activo con su cantidad y unidad en un medicamento."@es .
fmc:Laboratorio a owl:Class ; rdfs:subClassOf schema:MedicalOrganization ; rdfs:label "Laboratorio"@es .
fmc:ConceptoATC a owl:Class ; rdfs:subClassOf skos:Concept ;
  rdfs:label "Concepto ATC"@es ; rdfs:comment "Categoría de la clasificación ATC de la OMS."@es .
fmc:Excipiente a owl:Class ; rdfs:subClassOf skos:Concept ;
  rdfs:label "Excipiente"@es ; rdfs:comment "Excipiente de declaración obligatoria."@es .
fmc:ViaAdministracion a owl:Class ; rdfs:subClassOf skos:Concept ; rdfs:label "Vía de administración"@es .
fmc:FormaFarmaceutica a owl:Class ; rdfs:subClassOf skos:Concept ; rdfs:label "Forma farmacéutica"@es .
fmc:Interaccion a owl:Class ; rdfs:label "Interacción farmacológica"@es ;
  rdfs:comment "Interacción a nivel ATC (efecto + recomendación) del Nomenclátor de la AEMPS."@es .
fmc:NotaSeguridad a owl:Class ; rdfs:label "Nota de seguridad"@es ;
  rdfs:comment "Comunicación / alerta de seguridad de la AEMPS."@es .
fmc:SeccionDocumento a owl:Class ; rdfs:label "Sección de documento"@es .
fmc:SeccionProspecto a owl:Class ; rdfs:subClassOf fmc:SeccionDocumento ; rdfs:label "Sección de prospecto"@es .
fmc:SeccionFichaTecnica a owl:Class ; rdfs:subClassOf fmc:SeccionDocumento ; rdfs:label "Sección de ficha técnica"@es .
fmc:Introduccion a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Introducción"@es .
fmc:Indicaciones a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Indicaciones (para qué se utiliza)"@es .
fmc:ContraindicacionesAdvertencias a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Contraindicaciones y advertencias"@es .
fmc:Posologia a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Posología (cómo tomar)"@es .
fmc:EfectosAdversos a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Posibles efectos adversos"@es .
fmc:Conservacion a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Conservación"@es .
fmc:Composicion a owl:Class ; rdfs:subClassOf fmc:SeccionProspecto ; rdfs:label "Composición e información adicional"@es .

# ---- object properties ----
fmc:principioActivo a owl:ObjectProperty ; rdfs:label "principio activo"@es .
fmc:ingrediente a owl:ObjectProperty ; rdfs:label "ingrediente"@es .
fmc:atc a owl:ObjectProperty ; rdfs:label "clasificación ATC"@es ; rdfs:range fmc:ConceptoATC .
fmc:laboratorioTitular a owl:ObjectProperty ; rdfs:label "laboratorio titular"@es .
fmc:laboratorioComercializador a owl:ObjectProperty ; rdfs:label "laboratorio comercializador"@es .
fmc:via a owl:ObjectProperty ; rdfs:label "vía de administración"@es .
fmc:formaFarmaceutica a owl:ObjectProperty ; rdfs:label "forma farmacéutica"@es .
fmc:presentacion a owl:ObjectProperty ; rdfs:label "presentación"@es .
fmc:excipiente a owl:ObjectProperty ; rdfs:label "excipiente"@es .
fmc:vtm a owl:ObjectProperty ; rdfs:label "moiety terapéutica (SNOMED CT)"@es .
fmc:snomedSustancia a owl:ObjectProperty ; rdfs:label "sustancia SNOMED CT (DCSA)"@es .
fmc:snomedProducto a owl:ObjectProperty ; rdfs:label "producto SNOMED CT (DCP)"@es .
fmc:snomedProductoForma a owl:ObjectProperty ; rdfs:label "producto+forma SNOMED CT (DCPF)"@es .
fmc:interaccion a owl:ObjectProperty ; rdfs:label "interacción"@es .
fmc:atcObjetivo a owl:ObjectProperty ; rdfs:label "grupo ATC con el que interacciona"@es .
fmc:gravedad a owl:ObjectProperty ; rdfs:label "gravedad de la interacción"@es .
fmc:notaSeguridad a owl:ObjectProperty ; rdfs:label "nota de seguridad"@es .
fmc:seccionProspecto a owl:ObjectProperty ; rdfs:label "sección de prospecto"@es .
fmc:seccionFichaTecnica a owl:ObjectProperty ; rdfs:label "sección de ficha técnica"@es .
fmc:mencionaFrecuencia a owl:ObjectProperty ; rdfs:label "menciona frecuencia de efecto"@es .

# ---- datatype properties ----
fmc:cantidad a owl:DatatypeProperty ; rdfs:label "cantidad"@es .
fmc:unidad a owl:DatatypeProperty ; rdfs:label "unidad"@es .
fmc:dosis a owl:DatatypeProperty ; rdfs:label "dosis"@es .
fmc:condicionPrescripcion a owl:DatatypeProperty ; rdfs:label "condición de prescripción"@es .
fmc:esGenerico a owl:DatatypeProperty ; rdfs:label "es genérico (EFG)"@es ; rdfs:range xsd:boolean .
fmc:esHuerfano a owl:DatatypeProperty ; rdfs:label "es medicamento huérfano"@es ; rdfs:range xsd:boolean .
fmc:esBiosimilar a owl:DatatypeProperty ; rdfs:label "es biosimilar"@es ; rdfs:range xsd:boolean .
fmc:trianguloNegro a owl:DatatypeProperty ; rdfs:label "triángulo negro (seguimiento adicional)"@es ; rdfs:range xsd:boolean .
fmc:requiereReceta a owl:DatatypeProperty ; rdfs:label "requiere receta"@es ; rdfs:range xsd:boolean .
fmc:comercializado a owl:DatatypeProperty ; rdfs:label "comercializado"@es ; rdfs:range xsd:boolean .
fmc:codigoNacional a owl:DatatypeProperty ; rdfs:label "código nacional (CN)"@es .
fmc:efecto a owl:DatatypeProperty ; rdfs:label "efecto de la interacción"@es .
fmc:recomendacion a owl:DatatypeProperty ; rdfs:label "recomendación ante la interacción"@es .
fmc:texto a owl:DatatypeProperty ; rdfs:label "texto de la sección"@es .
fmc:seccionTitulo a owl:DatatypeProperty ; rdfs:label "título de sección"@es .
fmc:seccionNumero a owl:DatatypeProperty ; rdfs:label "número de sección"@es .

# ---- frequency band concepts (EU QRD) ----
fmc:FrecuenciaEfecto a owl:Class ; rdfs:subClassOf skos:Concept ; rdfs:label "Frecuencia de efecto adverso"@es .
"""
    for fid, _pat, flabel in FREQ_TERMS:
        ttl += f'<{FREQ}{fid}> a fmc:FrecuenciaEfecto ; skos:prefLabel "{flabel}"@es ; rdfs:label "{flabel}"@es .\n'
    ttl += """
# ---- interaction severity concepts ----
fmc:GravedadInteraccion a owl:Class ; rdfs:subClassOf skos:Concept ; rdfs:label "Gravedad de interacción"@es .
"""
    for gid, glabel in [("contraindicada", "Asociación contraindicada"),
                        ("no-recomendada", "Asociación no recomendada"),
                        ("precaucion", "Usar con precaución / ajuste"),
                        ("informativa", "Interacción informativa")]:
        ttl += f'<{GRAV}{gid}> a fmc:GravedadInteraccion ; skos:prefLabel "{glabel}"@es ; rdfs:label "{glabel}"@es .\n'
    OUT_ONT.write_text(ttl, encoding="utf-8")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()
