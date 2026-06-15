#!/usr/bin/env python3
"""Run the W3C SPARQL 1.1 query-evaluation tests against rete and score coverage.

A conformance harness: for each mf:QueryEvaluationTest it builds a .rete from the
test data, runs the query through the rete CLI, and compares to the W3C expected
result (SRX/SRJ for SELECT/ASK; an RDF graph isomorphism for CONSTRUCT/DESCRIBE).

Modes (--mode): local | lazy | cache. local opens the file in memory; lazy and
cache range-read it over HTTP from a local Range-capable server (parity check —
the tiny datasets make timing meaningless, the point is identical results).

Usage: run.py --rete <bin> --suite <sparql11 dir> [--mode local] [--filter str]
"""
import argparse, json, subprocess, sys, tempfile, xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

import rdflib
from rdflib.compare import to_isomorphic

MF = rdflib.Namespace("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#")
QT = rdflib.Namespace("http://www.w3.org/2001/sw/DataAccess/tests/test-query#")
RS = "http://www.w3.org/2005/sparql-results#"


def query_form(q: str) -> str:
    # crude but reliable: first of these keywords outside comments
    import re
    s = re.sub(r"(?m)#.*$", "", q)
    for kw in ("CONSTRUCT", "ASK", "DESCRIBE", "SELECT"):
        if re.search(r"\b" + kw + r"\b", s, re.I):
            # SELECT may appear in a subquery before CONSTRUCT; check order
            return kw
    return "SELECT"


_XSD = "http://www.w3.org/2001/XMLSchema#"
_NUM = {_XSD + x for x in ("integer", "decimal", "double", "float", "int", "long",
                           "nonNegativeInteger", "positiveInteger", "short", "byte")}


RELAXED = False  # set by --relaxed: compare numeric-looking literals by value even when untyped


def _isnum(v):
    try:
        float(v); return True
    except (TypeError, ValueError):
        return False


def _lit_key(value, dt, lang):
    # Canonicalize so equal *values* compare equal regardless of lexical form:
    # numerics by float; plain ⇄ xsd:string; lang case-folded. With RELAXED, an
    # untyped numeric-looking literal also compares as a number — isolating the
    # "right value, missing datatype" failures from genuinely wrong answers.
    if dt in _NUM or (RELAXED and dt is None and not lang and _isnum(value)):
        try:
            return ("num", float(value))
        except ValueError:
            pass
    if (dt is None or dt == _XSD + "string") and not lang:
        return ("str", value)
    return ("lit", value, dt, (lang or "").lower())


def norm_solution(b: dict) -> frozenset:
    out = []
    for var, t in b.items():
        ty = t.get("type")
        if ty == "bnode":
            out.append((var, "bnode"))  # relaxed: bnode identity not compared
        elif ty in ("literal", "typed-literal"):
            out.append((var, _lit_key(t.get("value"), t.get("datatype"), t.get("xml:lang") or t.get("lang"))))
        else:
            out.append((var, "uri", t.get("value")))
    return frozenset(out)


def parse_srj(text: str):
    d = json.loads(text)
    if "boolean" in d:
        return ("ask", bool(d["boolean"]))
    rows = d.get("results", {}).get("bindings", [])
    return ("select", Counter(norm_solution(b) for b in rows))


def parse_srx(text: str):
    root = ET.fromstring(text)
    b = root.find(f"{{{RS}}}boolean")
    if b is not None:
        return ("ask", b.text.strip().lower() == "true")
    rows = []
    for res in root.findall(f"{{{RS}}}results/{{{RS}}}result"):
        sol = {}
        for binding in res.findall(f"{{{RS}}}binding"):
            name = binding.get("name")
            child = list(binding)[0]
            tag = child.tag.split("}")[-1]
            if tag == "uri":
                sol[name] = {"type": "uri", "value": child.text}
            elif tag == "bnode":
                sol[name] = {"type": "bnode", "value": child.text}
            else:  # literal
                sol[name] = {"type": "literal", "value": child.text or "",
                             "datatype": child.get("datatype"),
                             "xml:lang": child.get("{http://www.w3.org/XML/1998/namespace}lang")}
        rows.append(sol)
    return ("select", Counter(norm_solution(b) for b in rows))


def parse_expected(path: Path):
    t = path.read_text(encoding="utf-8")
    if path.suffix == ".srj":
        return parse_srj(t)
    if path.suffix in (".srx", ".xml"):
        return parse_srx(t)
    # CONSTRUCT/DESCRIBE result graph
    g = rdflib.Graph().parse(data=t, format="turtle" if path.suffix == ".ttl" else None)
    return ("graph", g)


def build_rete(rete, data_files, graph_data, tmp: Path) -> Path:
    """Assemble the test data into a .rete (named graphs via N-Quads)."""
    out = tmp / "t.rete"
    if graph_data:
        ds = rdflib.Dataset()
        for d in data_files:
            ds.default_context.parse(str(d), format="turtle")
        for giri, gfile in graph_data:
            ds.graph(rdflib.URIRef(giri)).parse(str(gfile), format="turtle")
        nq = tmp / "t.nq"
        nq.write_bytes(ds.serialize(format="nquads").encode())
        src = nq
    elif data_files:
        src = data_files[0] if len(data_files) == 1 else None
        if src is None:
            nt = tmp / "t.nt"
            g = rdflib.Graph()
            for d in data_files:
                g.parse(str(d), format="turtle")
            nt.write_bytes(g.serialize(format="nt").encode())
            src = nt
    else:
        src = tmp / "empty.nt"; src.write_text("")
    r = subprocess.run([rete, "build", str(src), "-o", str(out)], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError("build failed: " + r.stderr.strip()[:200])
    return out


def run_query(rete, retefile, query, form):
    args = [rete, "sparql", str(retefile), query]
    if form in ("SELECT", "ASK"):
        args.append("--json")
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError("query: " + r.stderr.strip()[:200])
    return r.stdout


def compare(form, expected, actual_text):
    kind, exp = expected
    if kind == "ask":
        got = json.loads(actual_text).get("boolean")
        return bool(got) == exp
    if kind == "select":
        rows = json.loads(actual_text).get("results", {}).get("bindings", [])
        return Counter(norm_solution(b) for b in rows) == exp
    if kind == "graph":
        g = rdflib.Graph().parse(data=actual_text, format="turtle")
        return to_isomorphic(g) == to_isomorphic(exp)
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rete", required=True)
    ap.add_argument("--suite", required=True)
    ap.add_argument("--filter", default="")
    ap.add_argument("--relaxed", action="store_true")
    ap.add_argument("--list", action="store_true",
                    help="print per-test status (and the error for err/FAIL)")
    args = ap.parse_args()
    global RELAXED
    RELAXED = args.relaxed
    rete, suite = args.rete, Path(args.suite)

    cats = {}
    total = Counter()
    for manifest in sorted(suite.rglob("manifest.ttl")):
        g = rdflib.Graph()
        try:
            g.parse(str(manifest), format="turtle")
        except Exception:
            continue
        base = manifest.parent
        cat = str(base.relative_to(suite))
        for t in g.subjects(rdflib.RDF.type, MF.QueryEvaluationTest):
            name = str(g.value(t, MF.name) or t)
            if args.filter and args.filter not in cat and args.filter not in name:
                continue
            action = g.value(t, MF.action)
            qf = g.value(action, QT.query)
            result = g.value(t, MF.result)
            if qf is None or result is None:
                continue
            qpath = base / Path(str(qf).split("/")[-1])
            rpath = base / Path(str(result).split("/")[-1])
            data = [base / str(d).split("/")[-1] for d in g.objects(action, QT.data)]
            gdata = []
            for gd in g.objects(action, QT.graphData):
                giri = g.value(gd, rdflib.RDFS.label) or g.value(gd, QT.graph)
                gf = g.value(gd, QT.graph) or gd
                gdata.append((str(giri), base / str(gf).split("/")[-1]))
            try:
                query = qpath.read_text(encoding="utf-8")
                form = query_form(query)
                with tempfile.TemporaryDirectory() as td:
                    rf = build_rete(rete, data, gdata, Path(td))
                    out = run_query(rete, rf, query, form)
                    ok = compare(form, parse_expected(rpath), out)
                status = "pass" if ok else "FAIL"
                detail = ""
            except Exception as e:
                status = "err"  # unsupported / parse-reject / build error
                detail = str(e)
            total[status] += 1
            cats.setdefault(cat, Counter())[status] += 1
            if args.list and status != "pass":
                line = f"  {status:4} [{cat}] {name}"
                if detail:
                    line += f"  — {detail[:160]}"
                print(line)

    print(f"{'category':32} pass  FAIL  err")
    for cat in sorted(cats):
        c = cats[cat]
        print(f"{cat:32} {c['pass']:>4} {c['FAIL']:>5} {c['err']:>4}")
    n = sum(total.values())
    print(f"\nTOTAL {n} tests: {total['pass']} pass · {total['FAIL']} fail · {total['err']} error/unsupported")
    if n:
        print(f"pass rate: {100*total['pass']/n:.1f}%")


if __name__ == "__main__":
    main()
