#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Build an N-Triples graph of the Fuero Juzgo manuscript tradition.

Sources (no crawling -- two curated documents + a public IIIF manifest):
  * Mónica Castillo Lluch, "Índice de fueros / Fuero Juzgo" (UNIL)
    https://people.unil.ch/monicacastillolluch/indice-de-fueros/fuero-juzgo/
  * her manuscript list PDF (Annexe 1 + Annexe 2, the latter = PhiloBiblon BETA)
    https://people.unil.ch/monicacastillolluch/files/2020/03/mss-FJ-1.pdf
  * PhiloBiblon (BETA texid 1191) -- manid / anaid, incipits, explicits, copyists
  * IIIF manifests of the digitized witnesses (BSB Cod.hisp.28, etc.)

The witness inventory is a fixed scholarly corpus, so it is encoded here directly
rather than scraped.  Emits data/fuero_juzgo/fuero_juzgo.nt ; the ontology lives
in data/fuero_juzgo/fjo.ttl and both are fed to `rete build`.
"""
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "fuero_juzgo", "fuero_juzgo.nt")
OUT_TTL = os.path.join(ROOT, "data", "fuero_juzgo", "fjo.ttl")

# The fjo: ontology (TBox) -- emitted alongside the data so one committed script
# regenerates the whole dataset (data/ is gitignored). Fed to `rete build` with
# the .nt so the schema travels inside the .rete.
ONTOLOGY_TTL = r'''@prefix fjo:  <https://w3id.org/fuero-juzgo/ontology#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix schema: <https://schema.org/> .

# Fuero Juzgo Ontology (fjo): a lightweight model for a medieval text tradition --
# an abstract Work, the manuscript / fragment Witnesses that transmit it, the
# printed and critical Editions, the scholarly Studies, and the People,
# Institutions and Places that connect them. Aligned with Dublin Core Terms,
# FOAF, SKOS and schema.org so it federates with Wikidata, PhiloBiblon, Biblissima.

fjo: a owl:Ontology ;
    dcterms:title "Fuero Juzgo Ontology"@en ;
    rdfs:label "Fuero Juzgo Ontology"@en ;
    dcterms:description "A model of the manuscript tradition, editions and studies of the Fuero Juzgo (the Romance vernacular translation of the Visigothic Liber Iudiciorum) and related law codes."@en ;
    dcterms:creator "Built for the rete playground from the Índice de fueros of Mónica Castillo Lluch (UNIL) and PhiloBiblon (BETA)." ;
    owl:versionInfo "1.0 (2026-07-06)" .

fjo:Work a owl:Class ; rdfs:label "Work"@en ;
    rdfs:comment "An abstract text (e.g. the Fuero Juzgo, the Liber Iudiciorum), independent of any physical carrier."@en .
fjo:LegalText a owl:Class ; rdfs:subClassOf fjo:Work ; rdfs:label "Legal text"@en ;
    rdfs:comment "A Work that is a code of law, fuero or legal compilation."@en .
fjo:Witness a owl:Class ; rdfs:label "Witness"@en ;
    rdfs:comment "A physical testimony (manuscript or fragment) that transmits a Work."@en .
fjo:Manuscript a owl:Class ; rdfs:subClassOf fjo:Witness ; rdfs:label "Manuscript"@en .
fjo:Fragment a owl:Class ; rdfs:subClassOf fjo:Witness ; rdfs:label "Fragment"@en ;
    rdfs:comment "A witness surviving as one or a few folios."@en .
fjo:Edition a owl:Class ; rdfs:label "Edition"@en ; skos:closeMatch schema:Book ;
    rdfs:comment "A printed or critical edition of a Work."@en .
fjo:Study a owl:Class ; rdfs:label "Study"@en ; skos:closeMatch schema:ScholarlyArticle ;
    rdfs:comment "A scholarly study (monograph, article, thesis) of a Work or Witness."@en .
fjo:Person a owl:Class ; rdfs:label "Person"@en ; owl:equivalentClass foaf:Person ;
    rdfs:comment "A translator, scribe/copyist, editor, scholar or historical figure."@en .
fjo:Institution a owl:Class ; rdfs:label "Institution"@en ; owl:equivalentClass foaf:Organization ;
    rdfs:comment "A library, archive or foundation that holds a Witness, or a publisher."@en .
fjo:Place a owl:Class ; rdfs:label "Place"@en ; owl:equivalentClass schema:Place .

fjo:translationOf a owl:ObjectProperty ; rdfs:label "translation of"@en ;
    rdfs:domain fjo:Work ; rdfs:range fjo:Work ;
    rdfs:comment "The subject Work is a translation of the object Work."@en .
fjo:witnessOf a owl:ObjectProperty ; rdfs:label "witness of"@en ;
    rdfs:domain fjo:Witness ; rdfs:range fjo:Work ; owl:inverseOf fjo:hasWitness .
fjo:hasWitness a owl:ObjectProperty ; rdfs:label "has witness"@en ;
    rdfs:domain fjo:Work ; rdfs:range fjo:Witness .
fjo:heldBy a owl:ObjectProperty ; rdfs:label "held by"@en ;
    rdfs:domain fjo:Witness ; rdfs:range fjo:Institution .
fjo:copiedBy a owl:ObjectProperty ; rdfs:label "copied by"@en ;
    rdfs:domain fjo:Witness ; rdfs:range fjo:Person ;
    rdfs:comment "The scribe who copied this witness."@en .
fjo:copyOf a owl:ObjectProperty ; rdfs:label "copy of"@en ;
    rdfs:domain fjo:Witness ; rdfs:range fjo:Witness ;
    rdfs:comment "This witness was copied from that (earlier) witness."@en .
fjo:editionOf a owl:ObjectProperty ; rdfs:label "edition of"@en ;
    rdfs:domain fjo:Edition ; rdfs:range fjo:Work .
fjo:editsWitness a owl:ObjectProperty ; rdfs:label "edits witness"@en ;
    rdfs:domain fjo:Edition ; rdfs:range fjo:Witness ;
    rdfs:comment "The witness whose text this edition establishes."@en .
fjo:editedBy a owl:ObjectProperty ; rdfs:label "edited by"@en ;
    rdfs:domain fjo:Edition ; rdfs:range fjo:Person .
fjo:author a owl:ObjectProperty ; rdfs:label "author"@en ; rdfs:range fjo:Person ;
    rdfs:comment "Author of a Study or Edition."@en .
fjo:studies a owl:ObjectProperty ; rdfs:label "studies"@en ; rdfs:domain fjo:Study ;
    rdfs:comment "The Work or Witness the Study is about."@en .
fjo:commissionedBy a owl:ObjectProperty ; rdfs:label "commissioned by"@en ; rdfs:range fjo:Person ;
    rdfs:comment "The person who ordered a Work to be made/translated."@en .
fjo:locatedIn a owl:ObjectProperty ; rdfs:label "located in"@en ;
    rdfs:domain fjo:Institution ; rdfs:range fjo:Place .

fjo:siglum a owl:DatatypeProperty ; rdfs:label "siglum / shelfmark"@en ;
    rdfs:domain fjo:Witness ; rdfs:range xsd:string .
fjo:languageVariety a owl:DatatypeProperty ; rdfs:label "language variety"@en ;
    rdfs:comment "Dialect / scripta of a witness (e.g. leonés occidental, asturiano, castellano)."@en .
fjo:dateText a owl:DatatypeProperty ; rdfs:label "date (text)"@en ;
    rdfs:comment "Human-readable date or date range."@en .
fjo:notBefore a owl:DatatypeProperty ; rdfs:label "not before"@en ; rdfs:range xsd:integer ;
    rdfs:comment "Earliest plausible year, for range queries."@en .
fjo:notAfter a owl:DatatypeProperty ; rdfs:label "not after"@en ; rdfs:range xsd:integer ;
    rdfs:comment "Latest plausible year, for range queries."@en .
fjo:century a owl:DatatypeProperty ; rdfs:label "century"@en ; rdfs:range xsd:integer .
fjo:incipit a owl:DatatypeProperty ; rdfs:label "incipit"@en .
fjo:prologueIncipit a owl:DatatypeProperty ; rdfs:label "prologue incipit"@en .
fjo:explicit a owl:DatatypeProperty ; rdfs:label "explicit"@en .
fjo:colophon a owl:DatatypeProperty ; rdfs:label "colophon"@en .
fjo:folios a owl:DatatypeProperty ; rdfs:label "folios / extent"@en .
fjo:note a owl:DatatypeProperty ; rdfs:label "note"@en .
fjo:philobiblonManid a owl:DatatypeProperty ; rdfs:label "PhiloBiblon BETA manid"@en ; rdfs:range xsd:integer .
fjo:philobiblonAnaid a owl:DatatypeProperty ; rdfs:label "PhiloBiblon BETA anaid"@en ; rdfs:range xsd:integer .
fjo:philobiblonWorkId a owl:DatatypeProperty ; rdfs:label "PhiloBiblon BETA texid"@en ; rdfs:range xsd:integer .
fjo:iiifManifest a owl:ObjectProperty ; rdfs:label "IIIF manifest"@en ;
    rdfs:comment "Self-hosted IIIF Presentation manifest (on our R2) for the digitized witness; IRI-valued so it renders as a viewer in the rete playground."@en .
fjo:sourceManifest a owl:ObjectProperty ; rdfs:label "source IIIF manifest"@en ;
    rdfs:comment "The original (external) IIIF manifest the R2 copy was mirrored from -- kept for provenance."@en .
fjo:image a owl:ObjectProperty ; rdfs:label "image"@en ; rdfs:subPropertyOf schema:image ;
    rdfs:comment "A representative image (IRI) that renders inline in the rete playground."@en .
fjo:onlineViewer a owl:ObjectProperty ; rdfs:label "online viewer"@en ;
    rdfs:comment "Human-facing digital-facsimile viewer page (IRI)."@en .
'''

FJO = "https://w3id.org/fuero-juzgo/ontology#"
ID = "https://w3id.org/fuero-juzgo/id/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"
DCT = "http://purl.org/dc/terms/"
FOAF = "http://xmlns.com/foaf/0.1/"
SKOS = "http://www.w3.org/2004/02/skos/core#"
WD = "http://www.wikidata.org/entity/"

INT = XSD + "integer"
URI = XSD + "anyURI"

_ctrl = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")


def esc(s):
    s = _ctrl.sub(" ", str(s))
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


LINES = []


def out_iri(s, p, o):
    LINES.append("<%s> <%s> <%s> ." % (s, p, o))


def out_lit(s, p, o, lang=None):
    if o is None or o == "":
        return
    if lang:
        LINES.append('<%s> <%s> "%s"@%s .' % (s, p, esc(o), lang))
    else:
        LINES.append('<%s> <%s> "%s" .' % (s, p, esc(o)))


def out_typed(s, p, o, dt):
    if o is None or o == "":
        return
    LINES.append('<%s> <%s> "%s"^^<%s> .' % (s, p, esc(o), dt))


def label(s, text, lang="es"):
    out_lit(s, RDFS + "label", text, lang)


def a(s, cls):
    out_iri(s, RDF + "type", FJO + cls)


# ---------------------------------------------------------------------------
#  Wikidata QIDs (verified) -- owl:sameAs to the wider LOD cloud / playground
#  `wikidata` datasets.  Filled from a verification pass; only confident IDs.
# ---------------------------------------------------------------------------
WDMAP = {  # owl:sameAs -- the QID IS this exact entity
    # works
    "work/fuero-juzgo": "Q765908",
    "work/liber-iudiciorum": "Q1246455",
    # institutions
    "org/bsb": "Q256507", "org/bnf": "Q193563", "org/bne": "Q750403",
    "org/rae": "Q11993457", "org/bodleian": "Q82133", "org/hsa": "Q2420849",
    "org/escorial": "Q3848863", "org/loc": "Q131454", "org/kb-se": "Q953058",
    "org/kb-dk": "Q867885", "org/bnp": "Q245966", "org/lazaro-galdiano": "Q933136",
    "org/march": "Q18669971", "org/museo-pobo-galego": "Q3089563",
    "org/amm": "Q125881048",
    # persons
    "person/zeumer": "Q98150", "person/llorente": "Q1710349",
    "person/lopez-ferreiro": "Q3320987", "person/menendez-pidal": "Q381953",
    "person/burriel": "Q3616503", "person/alfonso-x": "Q47595",
    "person/jovellanos": "Q282498",
    # places
    "place/munchen": "Q1726", "place/paris": "Q90", "place/madrid": "Q2807",
    "place/oxford": "Q34217", "place/new-york": "Q60", "place/escorial": "Q371748",
    "place/washington": "Q61", "place/stockholm": "Q1754", "place/copenhagen": "Q1748",
    "place/lisboa": "Q597", "place/toledo": "Q5836", "place/murcia": "Q12225",
    "place/valladolid": "Q8356", "place/salamanca": "Q15695", "place/sevilla": "Q8717",
    "place/palma": "Q8826", "place/santiago": "Q14314",
}
# rdfs:seeAlso -- the QID is the parent institution, not the specific library/archive
WDSEE = {
    "org/bus": "Q308963",        # Universidad de Salamanca (its Biblioteca General Histórica)
    "org/us": "Q1229996",        # Universidad de Sevilla
    "org/santa-cruz": "Q768224", # Universidad de Valladolid
    "org/toledo-abc": "Q1123180",  # Catedral de Toledo (holds the Archivo y Biblioteca Capitulares)
    "org/escorial-monastery": "Q28471",  # Monasterio de El Escorial (context)
}
WD_SITELINK = "https://www.wikidata.org/wiki/"

# IIIF manifests + representative images + viewer pages, keyed by witness num.
# All URLs verified to resolve (2026-07); {num: {manifest, image, viewer}}.
MDZ_MAN = "https://api.digitale-sammlungen.de/iiif/presentation/v2/%s/manifest"
MDZ_IMG = "https://api.digitale-sammlungen.de/iiif/image/v2/%s/full/700,/0/default.jpg"
RBME_MAN = "https://rbdigital.realbiblioteca.es/files/manifests/esc_%s.json"
RBME_IMG = "https://imagenes.patrimonionacional.es/iiif/2/%s%%2F0001.jpg/full/700,/0/default.jpg"

IIIF = {
    9: {"manifest": MDZ_MAN % "bsb00160754", "image": MDZ_IMG % "bsb00160754_00015",
        "viewer": "https://www.digitale-sammlungen.de/de/view/bsb00160754"},   # BSB Cod.hisp. 28
    32: {"manifest": MDZ_MAN % "bsb00094631", "image": MDZ_IMG % "bsb00094631_00013",
         "viewer": "https://www.digitale-sammlungen.de/en/view/bsb00094631"},  # BSB Cod.hisp. 6
    8: {"manifest": "https://gallica.bnf.fr/iiif/ark:/12148/btv1b10033228s/manifest.json",
        "image": "https://gallica.bnf.fr/iiif/ark:/12148/btv1b10033228s/f1/full/,700/0/native.jpg",
        "viewer": "https://gallica.bnf.fr/ark:/12148/btv1b10033228s"},          # BnF Espagnol 256
    17: {"manifest": "https://iiif.bodleian.ox.ac.uk/iiif/manifest/aa0dadc7-b2e9-44fa-9b1c-4d25760d5297.json",
         "image": "https://iiif.bodleian.ox.ac.uk/iiif/image/157f81d2-3b9d-4451-9b10-e1fb481dccc1/full/700,/0/default.jpg",
         "viewer": "https://digital.bodleian.ox.ac.uk/objects/aa0dadc7-b2e9-44fa-9b1c-4d25760d5297/"},  # Holkham misc. 46
    16: {"viewer": "https://bnedigital.bne.es/bd/es/viewer?id=7c8c0ad6-8e0a-4e6a-a7b3-f387e6f7e049"},  # BNE VITR/17/10 (no IIIF)
}
# Escorial (RBME "Real Biblioteca Digital") IIIF set: witness num -> shelfmark (dashes).
RBME = {14: "Z-III-6", 18: "Z-III-21", 6: "Z-III-18", 19: "P-II-17",
        20: "M-II-18", 15: "M-III-5", 26: "Z-II-9", 25: "d-III-18"}
for _n, _sh in RBME.items():
    IIIF[_n] = {"manifest": RBME_MAN % _sh, "image": RBME_IMG % _sh,
                "viewer": "https://rbme.patrimonionacional.es/s/rbme"}

# Human viewer URLs from the sources (no IIIF confirmed) keyed by num
VIEWERS = {
    12: "https://www.archivodemurcia.es/p_pandora4/viewer.vm?id=1413967&view=archivo&lang=es",
}

# Biblissima+ entity links (owl:sameAs), keyed by witness num -- the direct
# bridge to the `biblissima` playground dataset / data.biblissima.fr.
BIBLISSIMA = {
    8: "https://data.biblissima.fr/entity/Q44277",    # BnF Espagnol 256
    32: "https://data.biblissima.fr/entity/Q264558",  # BSB Cod.hisp. 6
}

# ---- Self-hosted IIIF on R2 -------------------------------------------------
# Every digitized witness' full IIIF (manifest + all page images) is mirrored
# into our R2 bucket by scripts/fuero_juzgo_iiif_mirror.py. Point iiifManifest +
# image at OUR copy (renders in the playground AND is downloadable) and keep the
# original external manifest as fjo:sourceManifest for provenance.
R2IIIF = "https://data.graphplaza.com/fuero_juzgo/iiif"
MIRROR = {6: "Z-III-18", 8: "bnf-esp-256", 9: "bsb00160754", 14: "Z-III-6",
          15: "M-III-5", 17: "bodleian-holkham-46", 18: "Z-III-21", 19: "P-II-17",
          20: "M-II-18", 25: "d-III-18", 26: "Z-II-9", 32: "bsb00094631"}
for _n, _wk in MIRROR.items():
    ent = IIIF.setdefault(_n, {})
    if ent.get("manifest"):
        ent["sourceManifest"] = ent["manifest"]      # keep the external file
    ent["manifest"] = "%s/%s/manifest.json" % (R2IIIF, _wk)
    ent["image"] = "%s/%s/thumb.jpg" % (R2IIIF, _wk)  # small self-hosted thumbnail


def wd_sameas(key, subj):
    q = WDMAP.get(key)
    if q:
        out_iri(subj, OWL + "sameAs", WD + q)
    q2 = WDSEE.get(key)
    if q2:
        out_iri(subj, RDFS + "seeAlso", WD_SITELINK + q2)


# ===========================================================================
#  WORKS
# ===========================================================================
W_FJ = ID + "work/fuero-juzgo"
W_LI = ID + "work/liber-iudiciorum"
W_CAT = ID + "work/libre-jutge"

a(W_FJ, "LegalText")
label(W_FJ, "Fuero Juzgo")
out_lit(W_FJ, RDFS + "label", "Fuero Juzgo", "en")
out_lit(W_FJ, DCT + "title", "Fuero Juzgo")
out_lit(W_FJ, SKOS + "altLabel", "Fuero juzgo o libro de los jueces")
out_lit(W_FJ, SKOS + "altLabel", "Forum iudicum")
out_lit(W_FJ, DCT + "description",
        "Traducción romance (castellano / leonés) del Liber Iudiciorum visigótico, "
        "hecha hacia 1260, en tiempos de Fernando III. Transmitida por 46 manuscritos "
        "censados en PhiloBiblon (más un fragmento gallego, tres testimonios de la BOOST "
        "y dos versiones catalanas).", "es")
out_typed(W_FJ, FJO + "philobiblonWorkId", "1191", INT)
out_lit(W_FJ, DCT + "language", "es")
out_typed(W_FJ, FJO + "dateText", "1260 ca. (traducción)", XSD + "string")
out_iri(W_FJ, FJO + "translationOf", W_LI)
out_iri(W_FJ, FJO + "commissionedBy", ID + "person/fernando-iii")
out_iri(W_FJ, RDFS + "seeAlso", "https://people.unil.ch/monicacastillolluch/indice-de-fueros/fuero-juzgo/")
wd_sameas("work/fuero-juzgo", W_FJ)

a(W_LI, "LegalText")
label(W_LI, "Liber Iudiciorum")
out_lit(W_LI, RDFS + "label", "Liber Iudiciorum", "en")
out_lit(W_LI, SKOS + "altLabel", "Lex Visigothorum")
out_lit(W_LI, SKOS + "altLabel", "Forum Iudicum")
out_lit(W_LI, DCT + "description",
        "Código legal visigótico promulgado en el siglo VII (recensión Vulgata "
        "atribuida al concilio y a los reyes godos). Fuente latina del Fuero Juzgo.", "es")
out_lit(W_LI, DCT + "language", "la")
wd_sameas("work/liber-iudiciorum", W_LI)

a(W_CAT, "LegalText")
label(W_CAT, "Libre jutge")
out_lit(W_CAT, DCT + "description",
        "Versión catalana antigua del Liber Iudiciorum, conservada en fragmentos "
        "del siglo XII (los más antiguos testimonios romances de la ley visigótica).", "es")
out_lit(W_CAT, DCT + "language", "ca")
out_iri(W_CAT, FJO + "translationOf", W_LI)


# ===========================================================================
#  PLACES
# ===========================================================================
PLACES = {
    "santiago": "Santiago de Compostela", "madrid": "Madrid", "lisboa": "Lisboa",
    "escorial": "San Lorenzo de El Escorial", "new-york": "Nueva York",
    "paris": "París", "munchen": "Múnich", "washington": "Washington D. C.",
    "toledo": "Toledo", "murcia": "Murcia", "oxford": "Oxford",
    "stockholm": "Estocolmo", "copenhagen": "Copenhague", "palma": "Palma de Mallorca",
    "salamanca": "Salamanca", "sevilla": "Sevilla", "valladolid": "Valladolid",
    "seu-urgell": "La Seu d'Urgell", "burgos": "Burgos", "bejar": "Béjar",
}
for k, name in PLACES.items():
    s = ID + "place/" + k
    a(s, "Place")
    label(s, name)
    wd_sameas("place/" + k, s)


# ===========================================================================
#  INSTITUTIONS
# ===========================================================================
# key -> (display name, place key)
ORGS = {
    "museo-pobo-galego": ("Museo do Pobo Galego (fondo Blanco Cicerón)", "santiago"),
    "rae": ("Real Academia Española", "madrid"),
    "bnp": ("Biblioteca Nacional de Portugal", "lisboa"),
    "escorial": ("Real Biblioteca del Monasterio de El Escorial", "escorial"),
    "hsa": ("Hispanic Society of America", "new-york"),
    "bnf": ("Bibliothèque nationale de France", "paris"),
    "bsb": ("Bayerische Staatsbibliothek", "munchen"),
    "loc": ("Library of Congress", "washington"),
    "toledo-abc": ("Archivo y Biblioteca Capitulares de Toledo", "toledo"),
    "amm": ("Archivo Municipal de Murcia", "murcia"),
    "bne": ("Biblioteca Nacional de España", "madrid"),
    "bodleian": ("Bodleian Library", "oxford"),
    "lazaro-galdiano": ("Fundación Lázaro Galdiano", "madrid"),
    "kb-se": ("Kungliga biblioteket (Biblioteca Nacional de Suecia)", "stockholm"),
    "kb-dk": ("Det Kongelige Bibliotek (Biblioteca Real de Dinamarca)", "copenhagen"),
    "march": ("Fundación Bartolomé March", "palma"),
    "bus": ("Biblioteca General Histórica de la Universidad de Salamanca", "salamanca"),
    "us": ("Biblioteca de la Universidad de Sevilla", "sevilla"),
    "santa-cruz": ("Biblioteca Histórica de Santa Cruz (Universidad de Valladolid)", "valladolid"),
    "seu-urgell": ("Arxiu Capitular de la Seu d'Urgell", "seu-urgell"),
    "burgos-am": ("Archivo Municipal de Burgos", "burgos"),
    "bejar-am": ("Archivo Municipal de Béjar", "bejar"),
}
for k, (name, pk) in ORGS.items():
    s = ID + "org/" + k
    a(s, "Institution")
    label(s, name)
    out_iri(s, FJO + "locatedIn", ID + "place/" + pk)
    wd_sameas("org/" + k, s)


# ===========================================================================
#  PERSONS
# ===========================================================================
# key -> (display name, role note)
PERSONS = {
    "anonimo": ("Anónimo", "Traductor del Fuero Juzgo"),
    "fernando-iii": ("Fernando III de Castilla", "Rey que impulsó la traducción (según Castillo Lluch)"),
    "sisenando": ("Sisenando", "Rey visigodo; el IV Concilio de Toledo (633) se celebró en su presencia (citado en los incipits)"),
    "alfonso-x": ("Alfonso X de Castilla", "Escritorio real vinculado al ms. de la Library of Congress"),
    "pedro-martiz-gallego": ("Pedro Martiz Gallego", "Copista del Escorial Z.III.6 (colofón)"),
    "pedro-gonzalo-rodriguez": ("Pedro Gonzalo Rodríguez", "Copista del RAE 293 (1289)"),
    "santiago-palomares": ("Francisco Javier de Santiago y Palomares", "Copista y calígrafo del siglo XVIII (BNE 683, 1681)"),
    "torcuato-torio": ("Torcuato Torío de la Riva", "Calígrafo, copista del ms. de Valladolid (1780)"),
    "burriel": ("Andrés Marcos Burriel", "Jesuita; corrigió la copia BNE 683 (1755)"),
    "villadiego": ("Alonso de Villadiego Vascuñana y Montoya", "Jurista, primer editor (1600)"),
    "jovellanos": ("Gaspar Melchor de Jovellanos", "Ilustrado; promotor e impulsor del proyecto de edición académica del Fuero Juzgo por la RAE (según García Martín 2016). Veía en el texto la raíz medieval del derecho español para el diseño de una nueva constitución."),
    "llorente": ("Juan Antonio Llorente", "Reeditor del Fuero Juzgo (1792)"),
    "reguera": ("Juan de la Reguera Valdelomar", "Autor del extracto de 1798"),
    "lopez-ferreiro": ("Antonio López Ferreiro", "Historiador; editor (1895)"),
    "zeumer": ("Karl Zeumer", "Editor de las Leges Visigothorum (MGH, 1902)"),
    "garcia-lopez": ("Yolanda García López", "Estudiosa de la Lex Wisigothorum (1996)"),
    "mence-caster": ("Corinne Mencé-Caster", "Editora del Escorial Z.III.6 (tesis, 1996)"),
    "orazi": ("Verónica Orazi", "Editora del Escorial Z.III.21 (1997)"),
    "perona": ("José Perona", "Coordinador de la edición del Códice Murciano (2002)"),
    "castillo-lluch": ("Mónica Castillo Lluch", "Lingüista (UNIL); autora del Índice de fueros"),
    "pichel": ("Ricardo Pichel Gotérrez", "Coautor del estudio del códice López Ferreiro (2015)"),
    "romero-cambron": ("Ángeles Romero Cambrón", "Editora parcial del Holkham misc. 46 (2016)"),
    "garcia-martin": ("José María García Martín", "Estudioso de la edición académica del FJ (2016)"),
    "rivas-zancarron": ("Manuel Rivas Zancarrón", "Transcriptor de varios mss. de la RAE"),
    "jonxis-henkemans": ("Wilhelmina Jonxis-Henkemans", "Editora del texto y concordancia de la HSA B2567"),
    "craddock": ("Jerry Craddock", "Filólogo; estudio de la HSA B2567"),
    "diez-revenga": ("Pilar Díez de Revenga", "Estudiosa de la lengua del Códice Murciano"),
    "fernandez-llera": ("Víctor Fernández Llera", "Autor de la Gramática y vocabulario del Fuero Juzgo (1929)"),
    "galindo-vera": ("León Galindo y de Vera", "Autor (1863)"),
    "gessner": ("Emil Gessner", "Autor de Das Altleonesische (1867)"),
    "menendez-pidal": ("Ramón Menéndez Pidal", "Filólogo; El dialecto leonés (1906)"),
    "rodriguez-rodriguez": ("Manuel Rodríguez y Rodríguez", "Autor (1905)"),
    "staaff": ("Erik Staaff", "Autor del estudio sobre el leonés (1907)"),
    "mundo": ("Anscari M. Mundó", "Editor del fragmento catalán (1984)"),
    "baraut": ("Cebrià Baraut", "Coeditor del fragmento catalán de la Seu d'Urgell"),
    "moran": ("Josep Moran", "Coeditor del fragmento catalán de la Seu d'Urgell"),
}
for k, (name, note) in PERSONS.items():
    s = ID + "person/" + k
    a(s, "Person")
    label(s, name)
    out_lit(s, FOAF + "name", name)
    if note:
        out_lit(s, FJO + "note", note, "es")
    wd_sameas("person/" + k, s)


# ===========================================================================
#  WITNESSES (Fuero Juzgo) -- Annexe 1 numbering 0..46 + BOOST + Catalan
# ===========================================================================
# Fields: num, cls, name, inst, siglum, lang, date_text, nb, na, manid, anaid,
#         copyist, title2, prologue, incipit, explicit, colophon, folios,
#         online, note, editor_note, copy_of(list)
WITNESSES = [
    dict(num=0, cls="Fragment", name="Códice López Ferreiro", inst="museo-pobo-galego",
         siglum="fondo Blanco Cicerón (4 folios)", lang="romance occidental",
         date_text="1200-1230 (fin del reinado de Alfonso IX)", nb=1200, na=1230,
         editor_note="Ed.: López Ferreiro (1895); Castillo Lluch y Pichel (2015). "
                     "Estudios: Otero (1959), Gómez España (2005).",
         note="Fragmento bilingüe latín-romance; no censado en PhiloBiblon."),
    dict(num=1, cls="Manuscript", name="RAE 49", inst="rae", siglum="RAE 49 (antes Campomanes)",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=1345, anaid=451,
         online="https://www.rae.es/biblioteca-digital",
         incipit="El primero livro del fazimiento delas lees.",
         editor_note="Transcripción inédita de Manuel Rivas Zancarrón. Galindo y de Vera (1863) lo creía anterior al de Murcia."),
    dict(num=2, cls="Manuscript", name="RAE 51", inst="rae", siglum="RAE 51 (antes Béjar)",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=1346, anaid=452,
         online="https://www.rae.es/biblioteca-digital",
         editor_note="Transcripción inédita de Manuel Rivas Zancarrón."),
    dict(num=3, cls="Manuscript", name="RAE 53", inst="rae", siglum="RAE 53 (antes Malpica 1)",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=1347, anaid=453,
         online="https://www.rae.es/biblioteca-digital",
         editor_note="Transcripción inédita de Manuel Rivas Zancarrón."),
    dict(num=4, cls="Manuscript", name="RAE 54", inst="rae", siglum="RAE 54 (antes Malpica 2)",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=1348, anaid=454,
         online="https://www.rae.es/biblioteca-digital"),
    dict(num=5, cls="Manuscript", name="Lisboa BN IL 111", inst="bnp", siglum="IL 111",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=3786, anaid=6186,
         folios="ff. 1ra-111rb (Sharrer)",
         prologue="Con cuedado del amor de xpo et con grant diligencia",
         explicit="nola puede dalli adelante de..."),
    dict(num=6, cls="Manuscript", name="Escorial Z.III.18", inst="escorial", siglum="Z.III.18",
         date_text="1260 ca. a quo – 1300 (PhB)", nb=1260, na=1300, manid=1163, anaid=459,
         title2="libro iudgo", folios="ff. 1ra-170va (Zarco)",
         incipit="Aqui conpieza el libro iudgo",
         prologue="Con cuidado del amor de ihu. xpo. e con grant diligencia de don Sysnando",
         explicit="no la pueden dalli adelantre demandar",
         note="Zarco: aprovechado (Escurialense 2.º) para la edición de la RAE (1815)."),
    dict(num=7, cls="Manuscript", name="HSA B2567", inst="hsa", siglum="B2567",
         lang="castellano con marcas leonesas", date_text="escritura gótica s. XIII-XV",
         nb=1260, na=1300, manid=1351, anaid=457, folios="ff. 1ra-93va (Faulhaber)",
         incipit="Titulo de las cartas leygales In primo libro",
         prologue="Con coydado del amor de christo & con gran deligencia de don sisnando muy glorioso Rey de ispania & de francia",
         explicit="Reciba cient & L azotes antel iuyz",
         editor_note="Ed. Jonxis-Henkemans y Craddock (1999); Text and Concordance (1990)."),
    dict(num=8, cls="Manuscript", name="BnF Esp. 256", inst="bnf", siglum="Espagnol 256",
         lang="leonés", date_text="2ª mitad del s. XIII", nb=1260, na=1300, manid=1352, anaid=458,
         editor_note="Transcripción inédita de José María García Martín (1991)."),
    dict(num=9, cls="Manuscript", name="BSB Cod.hisp. 28", inst="bsb", siglum="Cod.hisp. 28",
         lang="asturiano", date_text="s. XIII", nb=1260, na=1300, manid=1349, anaid=455,
         title2="El Fuero Juzgo", folios="180 hojas, 4º",
         incipit="Este libro se llama el Fuero Juzgo",
         explicit="e si non ouier onde los pague reciba L azotes antel juiz [8.12.3]",
         editor_note="Academia de la Llingua Asturiana (1994).",
         note="URN urn:nbn:de:bvb:12-bsb00160754-2; dominio público (CC PDM 1.0)."),
    dict(num=10, cls="Manuscript", name="Library of Congress Law MS F8", inst="loc",
         siglum="Law MS (de Ricci Suppl.) F8 = LAW MSS .f8 LL RBR",
         date_text="1260 ca. a quo – 1400 (Escritorio real de Alfonso X?)", nb=1260, na=1400,
         manid=3002, anaid=3730, copyist="alfonso-x", folios="pp. 1-211 (Faulhaber)",
         prologue="Con cuydado del amor de xpo e con grant diligencia de don sisnando",
         explicit="la pueden de ali adelante demandar. Finis",
         note="Texto original termina p. 205b; completado pp. 206-11 en mano del s. XVIII."),
    dict(num=11, cls="Manuscript", name="Toledo ABC 43-10", inst="toledo-abc", siglum="43-10",
         date_text="1260 ca. a quo? – 1400? (PhB)", nb=1260, na=1400, manid=1357, anaid=465),
    dict(num=12, cls="Manuscript", name="Códice Murciano (AMM)", inst="amm",
         siglum="Archivo Municipal de Murcia", lang="castellano (copiado de un modelo con fuerte impronta leonesa)",
         date_text="1288", nb=1288, na=1288, manid=1350, anaid=456,
         editor_note="Ed. Perona et al. (2002); estudios de Díez de Revenga (2002, 2008)."),
    dict(num=13, cls="Manuscript", name="RAE 293", inst="rae", siglum="RAE 293 (antes Floranes)",
         date_text="1289-03-28", nb=1289, na=1289, manid=2870, anaid=3517,
         copyist="pedro-gonzalo-rodriguez", online="https://www.rae.es/biblioteca-digital",
         folios="ff. 1-119va (Castro & Onís)",
         incipit="El primero titulo es de la eleccion de los principes",
         colophon="Aqui acaba el libro julgo de leon. Hic liber est scriptus qui escripsit sit beneditus...",
         note="Incompleto (faltan 3 hojas)."),
    dict(num=14, cls="Manuscript", name="Escorial Z.III.6", inst="escorial", siglum="Z.III.6",
         lang="castellano", date_text="1290-1310", nb=1290, na=1310, manid=1355, anaid=462,
         copyist="pedro-martiz-gallego", folios="ff. 1r-207v (Zarco)",
         incipit="Los iudizios son dos El primero iudizio",
         colophon="finito libro redatur laus et gloria christo. Pedro martiz gualego me scripso dios le de la su gracia & lo meta en parayso amen.",
         editor_note="Ed. Mencé-Caster (1996, tesis, dir. Jean Roudil)."),
    dict(num=15, cls="Manuscript", name="Escorial M.III.5", inst="escorial", siglum="M.III.5",
         date_text="1290-1310 (PhB)", nb=1290, na=1310, manid=1354, anaid=461,
         folios="ff. 4ra-173ra (Zarco)",
         incipit="Titol de las cartas legales. El fazedor de la ley",
         prologue="Con cuydado del amor de xristo e con grand diligencia de don sisnando",
         explicit="l acotes antel iuez"),
    dict(num=16, cls="Manuscript", name="BNE Vit. 17-10", inst="bne", siglum="Vit. 17-10",
         date_text="Castilla, 1290 ca. – 1310 ca. (PhB)", nb=1290, na=1310, manid=4458, anaid=8230,
         online="http://bdh.bne.es/",
         editor_note="Edición en curso de Mónica Castillo Lluch."),
    dict(num=17, cls="Manuscript", name="Bodleian Holkham misc. 46", inst="bodleian",
         siglum="Holkham misc. 46", date_text="1290? – 1310? (PhB)", nb=1290, na=1310,
         manid=2850, anaid=3472, folios="ff. 1ra-130vb (Faulhaber)",
         prologue="CON COIDADO DEL AMOR DE xpo & con grant diligencia de don sisnando",
         explicit="non escapara dela setencia del Rey",
         colophon="Laus tibi sit xpe conpletur iudicus iste codex sanctorum retinens sibi iura priorum...",
         editor_note="Ed. parcial de Romero Cambrón (2016)."),
    dict(num=18, cls="Manuscript", name="Escorial Z.III.21", inst="escorial", siglum="Z.III.21",
         lang="leonés centro-occidental", date_text="2ª mitad del s. XIII", nb=1290, na=1310,
         manid=1337, anaid=464, folios="ff. 1rb-138ra (Zarco)",
         incipit="El primero titulo ye de la eleccion de los principes & del iusmamiento como deuen iulgar derecho",
         prologue="Con cuydado del amor de xpo. et con grant diligencia",
         colophon="Hic liber est scriptus qui scripsit sit benedictus. amen",
         editor_note="Ed. Orazi (1997)."),
    dict(num=19, cls="Manuscript", name="Escorial P.II.17", inst="escorial", siglum="P.II.17",
         lang="leonés occidental (extremo occidente)", date_text="1ª mitad del s. XIV", nb=1290, na=1310,
         manid=1353, anaid=460, folios="ff. 1ra-61vb (Zarco)",
         incipit="Titulo de las cartas legales en el primero liuro del compecamento de las leys",
         prologue="Com cuydado del amor de xristo et con gran diligentia",
         explicit="acotes ante iuyz", editor_note="Ed. Orazi (1997)."),
    dict(num=20, cls="Manuscript", name="Escorial M.II.18", inst="escorial", siglum="M.II.18",
         lang="leonés oriental", date_text="último cuarto del s. XIII", nb=1290, na=1310,
         manid=1341, anaid=466, folios="ff. 5ra-81va (Zarco)",
         explicit="non la puedan dalli adelante demandar",
         note="Acéfalo. Zarco: MS Escurialense 3.º de la ed. de la RAH (1815).",
         editor_note="Ed. Orazi (1997)."),
    dict(num=21, cls="Manuscript", name="Lázaro Galdiano M 20-11", inst="lazaro-galdiano",
         siglum="M 20-11", date_text="1300-1350 (PhB)", nb=1300, na=1350, manid=3128, anaid=3899,
         title2="Libro de las leyes fechas por los reyes godos"),
    dict(num=22, cls="Manuscript", name="BNE 21548", inst="bne", siglum="MSS/21548",
         date_text="1300-1350 (PhB)", nb=1300, na=1350, manid=4090, anaid=7270,
         title2="Libros de las leyes fachas por los reyes godos"),
    dict(num=23, cls="Manuscript", name="BNE 5975", inst="bne", siglum="MSS/5975",
         date_text="1300-1400 (PhB)", nb=1300, na=1400, manid=1359, anaid=468,
         folios="ff. 2r-86r (IGM)"),
    dict(num=24, cls="Manuscript", name="RAE 50", inst="rae", siglum="RAE 50 (antes San Bartolomé)",
         lang="castellano", date_text="escritura del s. XIII (PhB: 1300-1400)", nb=1300, na=1400,
         manid=1358, anaid=467, online="https://www.rae.es/biblioteca-digital",
         editor_note="Transcripción inédita de Manuel Rivas Zancarrón (2001)."),
    dict(num=25, cls="Manuscript", name="Escorial d.III.18", inst="escorial", siglum="d.III.18",
         date_text="1300-1400", nb=1300, na=1400, manid=1360, anaid=469, folios="ff. 1ra-137rb (Zarco)",
         incipit="Este es el libro primero & ffabla de los enssennamje[n]tos de las leys",
         colophon="Laus tibi sit christe conpretur iudicus iste codex sanctorum...",
         note="Zarco: consultado para la ed. de 1815 de la RAE (3.º escurialense)."),
    dict(num=26, cls="Manuscript", name="Escorial Z.II.9", inst="escorial", siglum="Z.II.9",
         date_text="1300-1400", nb=1300, na=1400, manid=1361, anaid=470, folios="ff. 1ra-177ra (Zarco)",
         prologue="Con cuydado del amor de xpo", explicit="e si no los pudiere auer. reciba antel iuez L.a acotes"),
    dict(num=27, cls="Manuscript", name="Toledo ABC 43-9", inst="toledo-abc", siglum="43-9",
         date_text="1300-1400", nb=1300, na=1400, manid=1363, anaid=472, note="Algunas glosas."),
    dict(num=28, cls="Manuscript", name="Kungliga Bibliotheket SP 16", inst="kb-se", siglum="SP 16",
         date_text="1300-1400", nb=1300, na=1400, manid=2918, anaid=3589, folios="ff. 5r-165 (Pensado)"),
    dict(num=29, cls="Manuscript", name="Kongelige Bibliotek GKS 1942", inst="kb-dk",
         siglum="Gaml. Kongl. Saml. 1942", date_text="1300-1400", nb=1300, na=1400,
         manid=2910, anaid=3573),
    dict(num=30, cls="Manuscript", name="Fundación Bartolomé March 20/5/4", inst="march",
         siglum="20/5/4", date_text="1300-1400", nb=1300, na=1400, manid=3268, anaid=4352,
         incipit="Titulo de las cartas legales enno primero liuro del fazemento de las leys",
         note="Localizado en Palma de Mallorca por PhiloBiblon y en Madrid por la BOOST."),
    dict(num=31, cls="Manuscript", name="Toledo ABC 15-37", inst="toledo-abc", siglum="15-37",
         date_text="1300-1400", nb=1300, na=1400, manid=1362, anaid=471),
    dict(num=32, cls="Manuscript", name="BSB Cod.hisp. 6", inst="bsb", siglum="Cod.hisp. 6",
         lang="agallegado", date_text="1400-1500 (PhB)", nb=1400, na=1500, manid=1366, anaid=475,
         explicit="los ninos non la podam demandar dali adelante"),
    dict(num=33, cls="Manuscript", name="BNE 2978", inst="bne", siglum="MSS/2978",
         date_text="1400-1500", nb=1400, na=1500, manid=1364, anaid=473,
         prologue="El primero titol ye de la election de los princepes e del insinnamiento como deuen iudgar",
         explicit="de tales pessonas non consientan tal yerro"),
    dict(num=34, cls="Manuscript", name="BNE 13632", inst="bne", siglum="MSS/13632",
         date_text="1500-1550", nb=1500, na=1550, manid=3166, anaid=3983, folios="ff. 3ra-va (tablas)"),
    dict(num=35, cls="Manuscript", name="Salamanca BU 35", inst="bus", siglum="Ms. 35",
         date_text="1500-1600", nb=1500, na=1600, manid=4067, anaid=7146, folios="ff. 1va-137vb (Faulhaber)",
         prologue="Con cuidado del amor de xpo y con gran deligencia de don sisnando",
         explicit="a tales personas non consientan tal yerro"),
    dict(num=36, cls="Manuscript", name="Sevilla BU 331/155", inst="us", siglum="331/155",
         date_text="1550-1600", nb=1550, na=1600, manid=4063, anaid=7139, folios="ff. 1v-208v (Faulhaber)",
         prologue="LOS IVDICIOS SON DOS: El primer judicio es el delos euangellos",
         explicit="en el año primero que nuestro sennor el bien auenturado Don Eurigio regno"),
    dict(num=37, cls="Manuscript", name="BNE 721", inst="bne", siglum="MSS/721",
         date_text="1600-1700", nb=1600, na=1700, manid=4296, anaid=7813, folios="ff. 55r-56r (Faulhaber)",
         title2="Ordenança y capitulación hecha con los conuersos de Toledo... sacada del Libro intitulado fuero Juzgo",
         note="Solo un extracto (ley de los judíos)."),
    dict(num=38, cls="Manuscript", name="BNE 6740", inst="bne", siglum="MSS/6740",
         date_text="1700-1800", nb=1700, na=1800, manid=4708, anaid=8829, folios="81 folios (pp. 1-1279 IGM)",
         online="http://bdh.bne.es/",
         title2="Fuero Juzgo hecho en tiempo del Rey Sisenando en el 4º concilio de Toledo..."),
    dict(num=39, cls="Manuscript", name="Valladolid Santa Cruz 5-6", inst="santa-cruz", siglum="Ms. 5-6",
         date_text="Valladolid, 1780-07-29", nb=1780, na=1780, manid=2571, anaid=2594,
         copyist="torcuato-torio"),
    dict(num=40, cls="Manuscript", name="BNE 683", inst="bne", siglum="MSS/683",
         date_text="1755", nb=1755, na=1755, manid=3566, anaid=4806, copyist="santiago-palomares",
         online="http://bdh.bne.es/",
         title2="Fuero Juzgo o Codigo de las leyes que los reyes godos promulgaron en España",
         note="Corregido por Andrés Marcos Burriel. Copia de Murcia (AMM) y de 3 mss. de Toledo.",
         copy_of=[12]),
    dict(num=41, cls="Manuscript", name="HSA B2713", inst="hsa", siglum="B2713",
         date_text="1725-1750", nb=1725, na=1750, manid=3840, anaid=6450, folios="ff. 1r-11r",
         prologue="Los judicios son dos El primero judicio es el de los evangelios",
         colophon="Finito libro reddatur laus et gloria Christo Pedro Martyr Gualego me scripso...",
         note="El prólogo parece independiente del texto."),
    dict(num=42, cls="Manuscript", name="BNE 1681", inst="bne", siglum="MSS/1681",
         date_text="1762-1764", nb=1762, na=1764, manid=3567, anaid=4807, copyist="santiago-palomares",
         online="http://bdh.bne.es/", title2="Leyes de los godos",
         prologue="Con cuidado del amor de Christo e con grant diligencia",
         explicit="reciba L.a azotes antel Juiz",
         note="Copia de los mss. del Escorial d.III.18 y P.II.17.",
         copy_of=[25, 19]),
    dict(num=43, cls="Manuscript", name="BNE 244", inst="bne", siglum="MSS/244",
         date_text="composite: 1260 ca.–1300; 1560-1590; 1590?-1610?", nb=1260, na=1610,
         manid=1356, anaid=463, folios="ff. 1ra-133ra (Faulhaber)",
         prologue="Con cuidado del amor de xpo e con gran diligencia de don sisnando",
         explicit="los Niños no la pueden dali adelante demandar",
         note="Manuscrito compuesto. Faltan las leyes nuevas de judíos."),
    dict(num=44, cls="Manuscript", name="BNE 5814", inst="bne", siglum="MSS/5814",
         date_text="composite: 1400-1500; 1290-1310", nb=1290, na=1500, manid=1365, anaid=474,
         title2="Fuero antiguo de los godos"),
    dict(num=45, cls="Manuscript", name="BNE 5774", inst="bne", siglum="MSS/5774",
         date_text="composite: 1500-1525; 1400-1500", nb=1400, na=1525, manid=3684, anaid=5758,
         folios="ff. 1ra-91vb (Faulhaber)",
         prologue="Con cuidado del amor de xristo E con grant diligencia",
         explicit="se non ouier onde los pague. Reciba. 150. azotes antel Juyz"),
    dict(num=46, cls="Manuscript", name="BNE 6705", inst="bne", siglum="MSS/6705",
         date_text="composite: 1600-1700; 1700-1800", nb=1600, na=1800, manid=3392, anaid=6251,
         folios="ff. 251r-259v", online="http://bdh.bne.es/", note="Incompleto."),
    # --- BOOST-only (Bibliography of Old Spanish Texts), not in PhiloBiblon ---
    dict(num="boost-burgos", cls="Manuscript", name="Burgos (García de Quevedo y Concellón)",
         inst="burgos-am", siglum="Archivo Municipal de Burgos", date_text="1289-03-28", nb=1289, na=1289,
         note="Censado en la BOOST (Faulhaber et al. 1984), no en PhiloBiblon."),
    dict(num="boost-bejar", cls="Manuscript", name="Béjar, Archivo Municipal 262",
         inst="bejar-am", siglum="262", date_text="s. XIII (junto con el Fuero de Béjar)", nb=1200, na=1300,
         note="Censado en la BOOST; encuadernado con el Fuero de Béjar. \"Fuero Juzgo dado por don Alonso el noveno en 11 de junio era de 1249, año 1211\" (nota del s. XVIII)."),
    dict(num="boost-escorial-diii8", cls="Manuscript", name="Escorial d.III.8",
         inst="escorial", siglum="d.III.8", date_text="1200-1300", nb=1200, na=1300,
         note="Fuero Juzgo; Enseñamientos de las leyes (bibliografía de Zarco Cuevas). Censado en la BOOST, no en PhiloBiblon."),
    # --- Catalan witnesses of the Libre jutge ---
    dict(num="cat-1", cls="Fragment", name="Fragment del Libre jutge (Mundó)", work="cat",
         siglum="2 folios", date_text="1180-1190", nb=1180, na=1190,
         editor_note="Ed. Anscari M. Mundó (1984), «Antic fragment del Libre jutge», Miscel·lània Aramon i Serra IV, 155-193.",
         note="Uno de los testimonios romances más antiguos de la ley visigótica."),
    dict(num="cat-2", cls="Fragment", name="Fragment del Liber iudiciorum de la Seu d'Urgell",
         inst="seu-urgell", work="cat", siglum="1 folio", date_text="1ª mitad del s. XII", nb=1100, na=1150,
         editor_note="Ed. Cebrià Baraut y Josep Moran (1996-1997 [2000]), Urgellia XIII, 7-35."),
]


def century_of(nb):
    if nb is None:
        return None
    return (nb // 100) + 1


for w in WITNESSES:
    num = w["num"]
    key = "ms/%s" % num
    s = ID + key
    a(s, w["cls"])
    label(s, w["name"])
    work = w.get("work", "fj")
    out_iri(s, FJO + "witnessOf", W_CAT if work == "cat" else W_FJ)
    out_iri((W_CAT if work == "cat" else W_FJ), FJO + "hasWitness", s)
    if w.get("inst"):
        out_iri(s, FJO + "heldBy", ID + "org/" + w["inst"])
    out_lit(s, FJO + "siglum", w.get("siglum"))
    out_lit(s, DCT + "identifier", w.get("siglum"))
    if w.get("lang"):
        out_lit(s, FJO + "languageVariety", w["lang"], "es")
    out_lit(s, FJO + "dateText", w.get("date_text"), "es")
    if w.get("nb") is not None:
        out_typed(s, FJO + "notBefore", str(w["nb"]), INT)
        out_typed(s, FJO + "century", str(century_of(w["nb"])), INT)
    if w.get("na") is not None:
        out_typed(s, FJO + "notAfter", str(w["na"]), INT)
    if w.get("manid"):
        out_typed(s, FJO + "philobiblonManid", str(w["manid"]), INT)
    if w.get("anaid"):
        out_typed(s, FJO + "philobiblonAnaid", str(w["anaid"]), INT)
    if w.get("copyist"):
        out_iri(s, FJO + "copiedBy", ID + "person/" + w["copyist"])
    out_lit(s, DCT + "title", w.get("title2"))
    out_lit(s, FJO + "prologueIncipit", w.get("prologue"))
    out_lit(s, FJO + "incipit", w.get("incipit"))
    out_lit(s, FJO + "explicit", w.get("explicit"))
    out_lit(s, FJO + "colophon", w.get("colophon"))
    out_lit(s, FJO + "folios", w.get("folios"))
    out_lit(s, FJO + "note", w.get("note"), "es")
    out_lit(s, FJO + "note", w.get("editor_note"), "es")
    for tgt in w.get("copy_of", []):
        out_iri(s, FJO + "copyOf", ID + "ms/%s" % tgt)
    # digitization / IIIF -- emit as IRIs so the playground auto-renders the
    # facsimile (image) and the IIIF viewer (manifest) cells.
    iiif = IIIF.get(num)
    if iiif:
        if iiif.get("manifest"):
            out_iri(s, FJO + "iiifManifest", iiif["manifest"])
        if iiif.get("sourceManifest"):
            out_iri(s, FJO + "sourceManifest", iiif["sourceManifest"])
        if iiif.get("image"):
            out_iri(s, FJO + "image", iiif["image"])
        if iiif.get("viewer"):
            out_iri(s, FJO + "onlineViewer", iiif["viewer"])
    if num in VIEWERS:
        out_iri(s, FJO + "onlineViewer", VIEWERS[num])
    if w.get("online"):
        out_iri(s, FJO + "onlineViewer", w["online"])
    if num in BIBLISSIMA:
        out_iri(s, OWL + "sameAs", BIBLISSIMA[num])
        out_iri(s, RDFS + "seeAlso", BIBLISSIMA[num])


# ===========================================================================
#  EDITIONS
# ===========================================================================
# key -> dict(name, year, work(fj/li), editor, place, publisher, url, note, edits(list of nums))
EDITIONS = [
    dict(key="villadiego-1600", name="Villadiego, Forus antiquus gothorum (1600)", year=1600,
         work="fj", editor="villadiego", place="madrid",
         note="Edita el texto de un ms. de la Santa Iglesia de Toledo (no es Vit. 17-10). Reeditada por Llorente (1792)."),
    dict(key="llorente-1792", name="Llorente, Fuero Juzgo o recopilación de las leyes de los wisi-godos (1792)",
         year=1792, work="fj", editor="llorente", place="madrid", publisher="Isidoro de Hernández Pacheco",
         url="https://archive.org/stream/leyesdelfueroju00llorgoog#page/n33/mode/1up",
         note="Reedición de la de Villadiego (1600) con puntuación corregida."),
    dict(key="reguera-1798", name="Reguera Valdelomar, Extracto de las leyes del Fuero juzgo (1798)",
         year=1798, work="fj", editor="reguera", place="madrid", publisher="Imprenta de la viuda e hijo de Marín",
         note="Extracto de las 578 leyes a partir de la ed. de Villadiego, adaptado a la lengua del s. XVIII."),
    dict(key="rae-1815", name="RAE, Fuero Juzgo en latín y castellano (1815 [1817])",
         year=1815, work="fj", place="madrid", publisher="Ibarra", promoter="jovellanos",
         date_range="1784-1817",
         note="Edición académica cotejada con los códices más antiguos. El proyecto duró de 1784 a "
              "1817: aunque la impresión de Ibarra estampa «1815», el año real de edición fue 1817, como "
              "demuestra García Martín (2016) leyendo los documentos del archivo de la RAE. Impulsada por "
              "Jovellanos (interés histórico: hallar en el FJ las raíces medievales del derecho para diseñar "
              "una constitución española); al final del proceso el objetivo pasó a ser lingüístico. El "
              "proyecto atravesó varias comisiones (bajas de Flores 1790 y Murillo 1791; destierro de "
              "Jovellanos y de Lardizábal), el período de Carlos IV / Godoy / Guerra de la Independencia y "
              "el período fernandino. Cotejó los mss. escurialenses Z.III.18 (2.º), M.II.18 (3.º) y d.III.18.",
         edits=[6, 20, 25]),
    dict(key="zeumer-1902", name="Zeumer, Leges Visigothorum (MGH, 1902)", year=1902,
         work="li", editor="zeumer", place=None, publisher="Hannover–Leipzig, Hahn",
         note="Monumenta Germaniae Historica, Leges I."),
    dict(key="garcia-lopez-1996", name="García López, Estudios críticos y literarios de la Lex Wisigothorum (1996)",
         year=1996, work="li", editor="garcia-lopez", publisher="Universidad de Alcalá"),
    dict(key="lopez-ferreiro-1895", name="López Ferreiro, Fueros municipales de Santiago y de su tierra (1895)",
         year=1895, work="fj", editor="lopez-ferreiro", place="santiago",
         note="El vol. II (p. 293-308) edita 6 folios de un códice bilingüe latín-romance del Liber Iudiciorum.",
         edits=[0]),
    dict(key="mence-1996", name="Mencé-Caster, Fuero Juzgo (ms. Z.III.6): introducción, transcripción y estudio (1996)",
         year=1996, work="fj", editor="mence-caster", place="paris",
         note="Tesis doctoral, Universidad de Paris XIII, dir. Jean Roudil.", edits=[14]),
    dict(key="orazi-1997", name="Orazi, El dialecto leonés antiguo... del Fuero Juzgo según el ms. Z.III.21 (1997)",
         year=1997, work="fj", editor="orazi", place="madrid", publisher="Universidad Europea-CEES",
         edits=[18]),
    dict(key="perona-2002", name="Perona et al., El Fuero juzgo (Códice Murciano) (2002)",
         year=2002, work="fj", editor="perona", place="murcia", publisher="Fundación Séneca, 2 vols.",
         edits=[12]),
    dict(key="castillo-pichel-2015", name="Castillo Lluch y Pichel, El códice López Ferreiro del Fuero juzgo (2015)",
         year=2015, work="fj", editor="castillo-lluch",
         note="Revue de linguistique romane 79, 123-168.", edits=[0]),
    dict(key="romero-cambron-2016", name="Romero Cambrón, El ms. Holkham misc. 46 de la Bodleian, testimonio del Fuero Juzgo: notas para su estudio y propuesta de edición parcial (2016)",
         year=2016, work="fj", editor="romero-cambron", publisher="Iberoamericana / Vervuert",
         url="https://people.unil.ch/monicacastillolluch/files/2019/10/resumen-Garc%C3%ADa-Mart%C3%ADn-y-Romero-Cambr%C3%B3n-2016.pdf",
         note="Edición parcial (pp. 209-275), en José María García Martín y Ángeles Romero Cambrón, "
              "El Fuero Juzgo: historia y lengua, Madrid-Frankfurt, Iberoamericana-Vervuert.",
         sections=[
             "Introducción codicológica (pp. 209-225).",
             "Edición del índice del ms. (pp. 225-259).",
             "Edición de la sección previa: prólogo y títulos I y II (pp. 260-262).",
             "Edición del libro II, I, 4-6 (pp. 262-266).",
             "Edición del libro XII, trece cánones (pp. 266-267).",
             "Edición del libro XII, IIII, 21-27, capítulos finales (pp. 268-269).",
             "Edición del concilio de Coyanza, títulos 1-3 (pp. 270-271).",
         ],
         edits=[17]),
]
for e in EDITIONS:
    s = ID + "edition/" + e["key"]
    a(s, "Edition")
    label(s, e["name"])
    out_iri(s, FJO + "editionOf", {"fj": W_FJ, "li": W_LI}[e["work"]])
    out_typed(s, FJO + "notBefore", str(e["year"]), INT)
    out_typed(s, DCT + "date", str(e["year"]), XSD + "gYear")
    if e.get("editor"):
        out_iri(s, FJO + "editedBy", ID + "person/" + e["editor"])
        out_iri(s, FJO + "author", ID + "person/" + e["editor"])
    if e.get("place"):
        out_iri(s, DCT + "spatial", ID + "place/" + e["place"])
    if e.get("promoter"):
        out_iri(s, FJO + "commissionedBy", ID + "person/" + e["promoter"])
    if e.get("date_range"):
        out_lit(s, FJO + "dateText", e["date_range"], "es")
    out_lit(s, DCT + "publisher", e.get("publisher"))
    out_lit(s, FJO + "note", e.get("note"), "es")
    if e.get("sections"):
        for sec in e["sections"]:
            out_lit(s, FJO + "note", sec, "es")
    if e.get("url"):
        out_iri(s, FJO + "onlineViewer", e["url"])
        out_iri(s, RDFS + "seeAlso", e["url"])
    for tgt in e.get("edits", []):
        out_iri(s, FJO + "editsWitness", ID + "ms/%s" % tgt)


# ===========================================================================
#  STUDIES
# ===========================================================================
STUDIES = [
    dict(key="castillo-2011", name="Castillo Lluch, Tel fils, tel père: Ferdinand III... (étude linguistique du Fuero juzgo) (2011)",
         year=2011, author="castillo-lluch", studies=W_FJ,
         url="https://www.academia.edu/11906259/", note="Habilitation à diriger des recherches, Paris IV."),
    dict(key="castillo-2012", name="Castillo Lluch, Las lenguas del Fuero juzgo... (I) (2012)",
         year=2012, author="castillo-lluch", studies=W_FJ,
         url="https://doi.org/10.4000/e-spania.20994", note="e-Spania 13."),
    dict(key="castillo-2016", name="Castillo Lluch, Las fechas del Fuero juzgo... (II) (2016)",
         year=2016, author="castillo-lluch", studies=W_FJ),
    dict(key="diez-revenga-2002", name="Díez de Revenga, Consideraciones sobre la lengua del Fuero Juzgo (Códice del A.M.M.) (2002)",
         year=2002, author="diez-revenga", studies=ID + "ms/12"),
    dict(key="fernandez-llera-1929", name="Fernández Llera, Gramática y vocabulario del Fuero Juzgo (1929)",
         year=1929, author="fernandez-llera", studies=W_FJ, publisher="Real Academia Española"),
    dict(key="galindo-vera-1863", name="Galindo y de Vera, Progreso y vicisitudes del idioma castellano... (1863)",
         year=1863, author="galindo-vera", studies=W_FJ),
    dict(key="gessner-1867", name="Gessner, Das Altleonesische (1867)", year=1867, author="gessner",
         studies=W_FJ, url="https://archive.org/details/dasaltleonesisch00gessuoft"),
    dict(key="menendez-pidal-1906", name="Menéndez Pidal, El dialecto leonés (1906)", year=1906,
         author="menendez-pidal", studies=W_FJ),
    dict(key="rodriguez-1905", name="Rodríguez y Rodríguez, Origen filológico del romance castellano... Fuero Juzgo (1905)",
         year=1905, author="rodriguez-rodriguez", studies=W_FJ,
         url="https://archive.org/details/origenfilolgic00rodruoft"),
    dict(key="staaff-1907", name="Staaff, Étude sur l'ancien dialecte léonais (1907)", year=1907,
         author="staaff", studies=W_FJ, url="https://archive.org/details/tudesurlancien00staauoft"),
    dict(key="garcia-martin-2016", name="García Martín, Bases para una crónica de la edición académica del Fuero Juzgo (2016)",
         year=2016, author="garcia-martin", studies=ID + "edition/rae-1815",
         publisher="Iberoamericana-Vervuert",
         url="https://people.unil.ch/monicacastillolluch/files/2019/10/resumen-Garc%C3%ADa-Mart%C3%ADn-y-Romero-Cambr%C3%B3n-2016.pdf",
         extra=[
             "En El Fuero Juzgo: historia y lengua (Madrid-Frankfurt, Iberoamericana-Vervuert), pp. 13-208: "
             "exposición (pp. 13-100) y edición de documentos del archivo de la RAE —actas y cartas— (pp. 100-202).",
             "Conclusión 1: la edición de la RAE no es de 1815 sino de 1817.",
             "Conclusión 2: en su origen primó el interés histórico por el texto (Jovellanos, Floridablanca), no el "
             "lingüístico; al final del proceso el objetivo pasó a subrayarse como lingüístico.",
             "Jovellanos como promotor colectivo y guía del proyecto (discursos de ingreso en la RAH 1870 y en la "
             "RAE 1871 sobre la unión del estudio de la legislación y el de la lengua y la historia).",
         ]),
]
for st in STUDIES:
    s = ID + "study/" + st["key"]
    a(s, "Study")
    label(s, st["name"])
    out_iri(s, FJO + "author", ID + "person/" + st["author"])
    out_typed(s, FJO + "notBefore", str(st["year"]), INT)
    out_typed(s, DCT + "date", str(st["year"]), XSD + "gYear")
    if st.get("studies"):
        out_iri(s, FJO + "studies", st["studies"])
    out_lit(s, DCT + "publisher", st.get("publisher"))
    out_lit(s, FJO + "note", st.get("note"), "es")
    for ex in st.get("extra", []):
        out_lit(s, FJO + "note", ex, "es")
    if st.get("url"):
        out_iri(s, RDFS + "seeAlso", st["url"])


# ---------------------------------------------------------------------------
os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(LINES) + "\n")
with open(OUT_TTL, "w", encoding="utf-8", newline="\n") as f:
    f.write(ONTOLOGY_TTL)
print("wrote %d triples -> %s" % (len(LINES), OUT))
print("wrote ontology -> %s" % OUT_TTL)
