#!/usr/bin/env python3
"""Build a temporal + multilingual subtitle graph (.nt) from a folder of .srt files
for ONE film in many languages (Tears of Steel — Blender "Mango", CC-BY 3.0).

Model
-----
  Work   schema:Movie                          — the film
  Track  su:SubtitleTrack   (one per language) — dct:language, rdfs:label, direction
  Cue    su:Cue             (one per subtitle) — su:inTrack, su:start/su:end (sec),
                                                 su:index, su:text "…"@lang, su:line →
  Line   su:Line            (a MOMENT in time) — su:start/su:end (from the English pivot),
                                                 su:index, and su:text "…"@lang for EVERY
                                                 language (the same utterance, all tongues)

Alignment is by time-overlap against the English original (its 76 cues define the Lines).
Cues are keyed by zero-padded start-ms so a time-range scan is a contiguous range read
(the same spatiotemporal trick as tracking.rete). Subtitles across languages are timed to
the same audio, so overlap ≈ translation-equivalence; a few tracks carry extra intro-credit
cues that overlap nothing and simply stay unaligned (honest)."""
import re, sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
RAW  = REPO / "data" / "subtitles" / "raw"
OUT  = REPO / "data" / "subtitles" / "tears_of_steel.nt"

SU     = "https://w3id.org/rete/subtitles#"
B      = "https://w3id.org/rete/subtitles/"
SCHEMA = "http://schema.org/"
DCT    = "http://purl.org/dc/terms/"
RDF    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS   = "http://www.w3.org/2000/01/rdf-schema#"
XSD    = "http://www.w3.org/2001/XMLSchema#"
PIVOT  = "en"

LANG_NAME = {
    "en":"English","es":"Spanish","de":"German","fr":"French","it":"Italian",
    "nl":"Dutch","ru":"Russian","pl":"Polish","cs":"Czech","hu":"Hungarian",
    "ja":"Japanese","zh-Hans":"Chinese (Simplified)","zh-Hant":"Chinese (Traditional)",
    "fa":"Persian","he":"Hebrew","el":"Greek","pt-BR":"Portuguese (Brazil)",
    "da":"Danish","id":"Indonesian","no":"Norwegian",
}
RTL = {"fa","he"}

TC = re.compile(r'(\d\d):(\d\d):(\d\d)[.,](\d{1,3})\s*-+>\s*(\d\d):(\d\d):(\d\d)[.,](\d{1,3})')
TAG = re.compile(r'<[^>]+>|\{[^}]*\}')          # <i>…</i>, ASS {\…} tags

def decode(path: Path) -> str:
    b = path.read_bytes()
    if   b[:2]==b'\xff\xfe': enc="utf-16-le"
    elif b[:2]==b'\xfe\xff': enc="utf-16-be"
    elif b[:3]==b'\xef\xbb\xbf': enc="utf-8-sig"
    else: enc="utf-8"
    return b.decode(enc, errors="replace")

def secs(h,m,s,ms) -> float:
    return int(h)*3600 + int(m)*60 + int(s) + int(ms.ljust(3,"0"))/1000.0

def clean(text: str) -> str:
    text = TAG.sub("", text)
    text = text.replace("​","").replace("﻿","")
    text = re.sub(r'\s+', ' ', text.replace("\r"," ").replace("\n"," ")).strip()
    return text

def parse_srt(path: Path):
    """-> list of dicts {index, start, end, text} in file order.

    Timecode-anchored so it survives irregular SRT (double-spaced files where a blank
    line sits between the timecode and its text, extra blank lines, etc.). A cue's text
    is every non-blank line between its timecode and the next, minus the trailing
    standalone integer (which is the *next* cue's index in standard SRT ordering)."""
    lines = decode(path).replace("\r\n","\n").replace("\r","\n").split("\n")
    tci = [i for i,l in enumerate(lines) if TC.search(l)]
    cues, n = [], 0
    for k, i in enumerate(tci):
        m = TC.search(lines[i])
        start = secs(*m.group(1,2,3,4)); end = secs(*m.group(5,6,7,8))
        j = tci[k+1] if k+1 < len(tci) else len(lines)
        seg = [l for l in lines[i+1:j] if l.strip() != ""]
        if seg and re.fullmatch(r'\d+', seg[-1].strip()):   # drop next cue's index number
            seg = seg[:-1]
        text = clean(" ".join(seg))
        if not text: continue
        n += 1
        cues.append({"index":n, "start":start, "end":end, "text":text})
    return cues

def overlap(a, b) -> float:
    return max(0.0, min(a["end"],b["end"]) - max(a["start"],b["start"]))

def best(cue, others):
    """the cue in `others` with max time-overlap, or None."""
    bo, bc = 0.0, None
    for o in others:
        ov = overlap(cue, o)
        if ov > bo: bo, bc = ov, o
    return bc if bo > 0 else None

# ---- N-Triples helpers ----
def esc(s: str) -> str:
    return (s.replace("\\","\\\\").replace('"','\\"')
             .replace("\n","\\n").replace("\r","").replace("\t","\\t"))
def iri(s):  return f"<{s}>"
def lit(s):  return f'"{esc(s)}"'
def tag(s,l):return f'"{esc(s)}"@{l}'
def typ(s,t):return f'"{esc(s)}"^^<{t}>'

def main():
    files = sorted(RAW.glob("tos.*.srt"))
    if not files:
        sys.exit("no tos.*.srt in "+str(RAW))
    tracks = {}                     # lang -> [cues]
    for f in files:
        lang = f.name[len("tos."):-len(".srt")]
        tracks[lang] = parse_srt(f)
    if PIVOT not in tracks:
        sys.exit("pivot language '%s' missing" % PIVOT)
    langs = list(tracks.keys())
    total_cues = sum(len(v) for v in tracks.values())

    out = []
    def t(s,p,o): out.append(f"{s} {p} {o} .")

    W = iri(B+"tears-of-steel")
    t(W, iri(RDF+"type"),        iri(SCHEMA+"Movie"))
    t(W, iri(SCHEMA+"name"),     lit("Tears of Steel"))
    t(W, iri(DCT+"title"),       lit("Tears of Steel"))
    t(W, iri(SCHEMA+"alternateName"), lit("Project Mango"))
    t(W, iri(SCHEMA+"datePublished"), typ("2012", XSD+"gYear"))
    t(W, iri(SCHEMA+"creator"),  lit("Blender Foundation"))
    t(W, iri(SCHEMA+"license"),  iri("https://creativecommons.org/licenses/by/3.0/"))
    t(W, iri(SCHEMA+"url"),      iri("https://mango.blender.org/"))
    t(W, iri(DCT+"source"),      iri("https://media.xiph.org/mango/subtitles/"))
    t(W, iri(SU+"languageCount"),typ(str(len(langs)), XSD+"integer"))
    t(W, iri(SU+"cueCount"),     typ(str(total_cues), XSD+"integer"))
    t(W, iri(RDFS+"comment"),
        lit("Tears of Steel — (CC) Blender Foundation, mango.blender.org, CC BY 3.0. "
            "Subtitles from media.xiph.org/mango/subtitles/. A temporal, multilingual "
            "subtitle graph: each Line is a moment in the film carrying the same utterance "
            "in %d languages; each Cue is one language's timed segment." % len(langs)))

    # tracks
    for lang in langs:
        T = iri(B+"track/"+lang)
        t(T, iri(RDF+"type"),   iri(SU+"SubtitleTrack"))
        t(T, iri(SU+"ofWork"),  W)
        t(W, iri(SU+"hasTrack"),T)
        t(T, iri(DCT+"language"), lit(lang))
        t(T, iri(RDFS+"label"), lit(LANG_NAME.get(lang, lang)))
        t(T, iri(SU+"cueCount"),typ(str(len(tracks[lang])), XSD+"integer"))
        t(T, iri(SU+"direction"), lit("rtl" if lang in RTL else "ltr"))

    def cue_iri(lang, c):
        return iri(B+"cue/%08d-%s" % (round(c["start"]*1000), lang))
    def line_iri(c):
        return iri(B+"line/%08d" % round(c["start"]*1000))

    # per-language cue nodes
    for lang in langs:
        Tr = iri(B+"track/"+lang)
        for c in tracks[lang]:
            C = cue_iri(lang, c)
            t(C, iri(RDF+"type"),  iri(SU+"Cue"))
            t(C, iri(SU+"inTrack"),Tr)
            t(C, iri(DCT+"language"), lit(lang))
            t(C, iri(SU+"index"),  typ(str(c["index"]), XSD+"integer"))
            t(C, iri(SU+"start"),  typ("%.3f"%c["start"], XSD+"decimal"))
            t(C, iri(SU+"end"),    typ("%.3f"%c["end"],   XSD+"decimal"))
            t(C, iri(SU+"text"),   tag(c["text"], lang))

    # Lines = English pivot cues; carry every language's overlapping text; link cues
    english = tracks[PIVOT]
    aligned_pairs = 0
    for e in english:
        L = line_iri(e)
        t(L, iri(RDF+"type"),  iri(SU+"Line"))
        t(L, iri(SU+"ofWork"), W)
        t(L, iri(SU+"index"),  typ(str(e["index"]), XSD+"integer"))
        t(L, iri(SU+"start"),  typ("%.3f"%e["start"], XSD+"decimal"))
        t(L, iri(SU+"end"),    typ("%.3f"%e["end"],   XSD+"decimal"))
        t(L, iri(RDFS+"label"),lit(e["text"]))
        t(L, iri(SU+"text"),   tag(e["text"], PIVOT))
        t(cue_iri(PIVOT, e), iri(SU+"line"), L)
        for lang in langs:
            if lang == PIVOT: continue
            bc = best(e, tracks[lang])
            if bc:
                t(L, iri(SU+"text"), tag(bc["text"], lang))
    # cue -> line links (each foreign cue to its best English moment)
    for lang in langs:
        if lang == PIVOT: continue
        for c in tracks[lang]:
            be = best(c, english)
            if be:
                t(cue_iri(lang, c), iri(SU+"line"), line_iri(be))
                aligned_pairs += 1

    OUT.write_text("\n".join(out)+"\n", encoding="utf-8")
    print(f"languages : {len(langs)}  ({', '.join(langs)})")
    print(f"cues      : {total_cues}")
    print(f"lines     : {len(english)} (English pivot)")
    print(f"aligned   : {aligned_pairs} foreign cues linked to a line")
    print(f"triples   : {len(out)}")
    print(f"-> {OUT}  ({OUT.stat().st_size/1024:.1f} KB)")

if __name__ == "__main__":
    main()
