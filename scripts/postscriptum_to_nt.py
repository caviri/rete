#!/usr/bin/env python3
"""Convert the Post Scriptum TEI-P5 letters (TEITOK / CLUL) to N-Triples.

Post Scriptum (teitok.clul.ul.pt/postscriptum) is a corpus of ~4,800 everyday
Portuguese & Spanish letters, 1500-1800. Each TEI file is one letter with a rich
correspDesc (sender, recipient, origin/destination places, date), a classification
(type, pragmatics, keyword terms), language, and a tokenised, modernised body.

Model (base https://teitok.clul.ul.pt/postscriptum/):
  letter/<PSID>  a ps:Letter
     dcterms:title ; ps:date ; dc:language ; ps:corpus (ES1500..PT1800) ;
     ps:sentBy -> person/<ref> ; ps:sentFrom (place literal) ;
     ps:receivedBy -> person/<ref> ; ps:sentTo (place literal) ;
     ps:letterType ; ps:pragmatics ; ps:keyword* ; ps:text (modernised) ;
     foaf:page -> the live TEITOK letter page (a URL-id)
  person/<ref>   a foaf:Person ; rdfs:label (name)   -> the correspondence network
"""
import os, re, sys, glob
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEI_DIR = os.path.join(HERE, "data", "postscriptum", "tei")
OUT = os.path.join(HERE, "data", "postscriptum", "postscriptum.nt")
SITE = "http://teitok.clul.ul.pt/postscriptum/index.php?action=file&id="

B = "https://teitok.clul.ul.pt/postscriptum/"
PS = B + "ns#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
DC = "http://purl.org/dc/elements/1.1/"
FOAF = "http://xmlns.com/foaf/0.1/"
T = "{http://www.tei-c.org/ns/1.0}"


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def lit(s, lang=None, dt=None):
    s = esc(str(s).strip())
    if dt:
        return f'"{s}"^^<{dt}>'
    return f'"{s}"@{lang}' if lang else f'"{s}"'


def slug(s):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", str(s)).strip("_")


def reflocal(ref):
    # pscdd:PLC1 -> PLC1 ; pscdd:AHN -> AHN
    return slug(ref.split(":")[-1]) if ref else ""


def word_text(w):
    ch = w.find(T + "choice")
    if ch is not None:
        for tag in ("reg", "expan"):
            e = ch.find(T + tag)
            if e is not None and (e.text or "").strip():
                return e.text.strip()
        return ""  # orig/abbr-only (no modernised form)
    return ("".join(w.itertext())).strip()


def body_text(root):
    body = root.find(f".//{T}body")
    if body is None:
        return ""
    words = [word_text(w) for w in body.iter(T + "w")]
    return " ".join(x for x in words if x)


def first(el, path):
    e = el.find(path)
    return e if e is not None else None


def main():
    files = sorted(glob.glob(os.path.join(TEI_DIR, "**", "*.xml"), recursive=True))
    out = open(OUT, "w", encoding="utf-8")
    w = lambda s, p, o: out.write(f"<{s}> <{p}> {o} .\n")
    persons = {}   # ref -> name
    n = 0
    for path in files:
        fn = os.path.basename(path)                 # PS4000_TEIP5.xml
        psid = fn.split("_")[0]                      # PS4000
        if not psid.upper().startswith("PS"):
            continue
        corpus = os.path.basename(os.path.dirname(path)).split("_")[0]  # ES1500
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError:
            continue
        s = f"{B}letter/{slug(psid)}"
        w(s, RDF + "type", f"<{PS}Letter>")
        w(s, FOAF + "page", f"<{SITE}{fn}>")
        if corpus:
            w(s, PS + "corpus", lit(corpus))
            lang = "es" if corpus.startswith("ES") else ("pt" if corpus.startswith("PT") else None)

        title = first(root, f".//{T}titleStmt/{T}title")
        if title is not None and (title.text or "").strip():
            tt = re.sub(r"\s+", " ", title.text).strip()
            w(s, DCT + "title", lit(tt, lang="es" if corpus.startswith("ES") else "pt"))
            w(s, RDFS + "label", lit(tt))

        tl = first(root, f".//{T}textLang")
        mainlang = tl.get("mainLang") if tl is not None else None
        if mainlang:
            w(s, DC + "language", lit(mainlang))

        # correspDesc: sent / received
        def party(typ, by_pred, place_pred, want_date):
            for ca in root.iter(T + "correspAction"):
                if ca.get("type") != typ:
                    continue
                pn = ca.find(T + "persName")
                if pn is not None:
                    name = re.sub(r"\s+", " ", "".join(pn.itertext())).strip()
                    ref = reflocal(pn.get("ref") or "")
                    if not ref and name:
                        ref = slug(name)
                    if ref:
                        pid = f"{B}person/{ref}"
                        w(s, by_pred, f"<{pid}>")
                        if name and ref not in persons:
                            persons[ref] = name
                pl = ca.find(T + "placeName")
                if pl is not None and (pl.text or "").strip():
                    w(s, place_pred, lit(re.sub(r"\s+", " ", pl.text).strip()))
                if want_date:
                    d = ca.find(T + "date")
                    if d is not None and d.get("when"):
                        w(s, PS + "date", lit(d.get("when")))
                break
        party("sent", PS + "sentBy", PS + "sentFrom", True)
        party("received", PS + "receivedBy", PS + "sentTo", False)

        # classification
        for cr in root.iter(T + "catRef"):
            scheme = (cr.get("scheme") or "")
            tgt = (cr.get("target") or "").split(":")[-1]
            if not tgt:
                continue
            if "ps_type" in scheme:
                w(s, PS + "letterType", lit(tgt))
            elif "ps_pragmatics" in scheme:
                w(s, PS + "pragmatics", lit(tgt))
        for term in root.iter(T + "term"):
            kw = (term.text or "").strip()
            if kw:
                w(s, PS + "keyword", lit(kw, lang="es" if corpus.startswith("ES") else "pt"))

        txt = body_text(root)
        if txt:
            w(s, PS + "text", lit(txt[:8000], lang=mainlang or None))
        n += 1
        if n % 1000 == 0:
            print(f"  {n} letters", flush=True)

    for ref, name in persons.items():
        p = f"{B}person/{ref}"
        out.write(f"<{p}> <{RDF}type> <{FOAF}Person> .\n")
        out.write(f'<{p}> <{RDFS}label> {lit(name)} .\n')
    out.close()
    print(f"DONE: {n} letters, {len(persons)} persons -> {OUT}")


if __name__ == "__main__":
    main()
