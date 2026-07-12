#!/usr/bin/env python3
"""Build a World Cup 2026 knowledge graph (.nt) from OpenFootball open data.

The 2026 tournament (Canada/USA/Mexico, 48 teams, 12 groups, Round of 32, 104
matches) is LIVE — this is a snapshot: 100 matches played (group + R32 + R16 +
QF), the 4 semi-finalists set, semis + final still to come (modelled as
scheduled fixtures). Results are AS RECORDED BY OpenFootball (community data) —
not independently verified. No prediction layer (FiveThirtyEight is retired and
no 2026 forecast time-series is available).
"""
from __future__ import annotations
import re, urllib.parse
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
RAW = REPO / "data" / "worldcup2026" / "raw"
OUT = REPO / "data" / "worldcup2026" / "worldcup2026.nt"

WC="https://w3id.org/rete/worldcup#"; B="https://w3id.org/rete/worldcup/2026/"
SC="http://schema.org/"; DCT="http://purl.org/dc/terms/"
RDF="http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS="http://www.w3.org/2000/01/rdf-schema#"; GEO="http://www.opengis.net/ont/geosparql#"
XSD="http://www.w3.org/2001/XMLSchema#"
MONTHS={"June":6,"July":7,"Jun":6,"Jul":7,"May":5}

_out=[]
def slug(s): return urllib.parse.quote(s.strip().replace(" ","_"), safe="")
def esc(s): return str(s).replace("\\","\\\\").replace('"','\\"').replace("\n"," ").replace("\r"," ").replace("\t"," ")
def iri(s,p,o): _out.append(f"<{s}> <{p}> <{o}> .")
def lit(s,p,v,dt=None,lang=None):
    o=f'"{esc(v)}"'+(f"^^<{dt}>" if dt else (f"@{lang}" if lang else "")); _out.append(f"<{s}> <{p}> {o} .")
def team_iri(n): return B+"team/"+slug(n)

# ---- dynamic groups (12 groups A-L) ----
GROUPS={}; TEAMS=[]
def parse_groups():
    for line in (RAW/"cup.txt").read_text(encoding="utf-8").splitlines():
        m=re.match(r"^\s*Group ([A-L])\s*\|\s*(.+)$", line)
        if m:
            names=re.split(r"\s{2,}", m[2].strip())
            names=[n.strip() for n in names if n.strip()]
            GROUPS[m[1]]=names; TEAMS.extend(names)
    TEAMS.sort(key=len, reverse=True)

def teams_in(pre):
    hits=[]
    for t in TEAMS:
        i=pre.find(t)
        while i!=-1: hits.append((i,t)); i=pre.find(t,i+1)
    hits.sort()
    out=[]
    for pos,t in hits:
        if any(pos>=p and pos+len(t)<=p+len(nm) and (p,nm)!=(pos,t) for p,nm in hits): continue
        out.append((pos,t))
    out.sort(); names=[t for _,t in out]
    return (names[0],names[1]) if len(names)>=2 else (None,None)

# ---- name normalization (full name first, surname later) ----
CANON={}
def build_canon(names):
    from collections import defaultdict
    g=defaultdict(list)
    for n in names: g[n.split()[-1] if n.split() else n].append(n)
    for _,grp in g.items():
        c=max(grp,key=len)
        for n in grp: CANON[n]=c
def scan_names():
    names=set()
    for path in ["cup.txt","cup_finals.txt"]:
        for line in (RAW/path).read_text(encoding="utf-8").splitlines():
            s=line.strip()
            if "@" in s: continue
            for m in re.finditer(r"([A-ZÀ-Ý][A-ZÀ-Ýa-zà-ÿ' .\-]+?)\s+\d+\+?\d*'", s):
                nm=m[1].strip(" .,")
                if len(nm)>=2 and nm.lower() not in ("pen","o.g","og","a.e.t","p"): names.add(nm)
    build_canon(names)

PLAYERS=set()
def player_iri(name):
    n=CANON.get(name.strip(), name.strip()); PLAYERS.add(n); return B+"player/"+slug(n)

# ---- static ----
def emit_static():
    iri(B+"WC2026", RDF, SC+"SportsEvent"); iri(B+"WC2026", RDF, WC+"Tournament")
    lit(B+"WC2026", SC+"name","FIFA World Cup 2026"); lit(B+"WC2026", DCT+"date","2026")
    lit(B+"WC2026", SC+"location","Canada, USA & Mexico")
    lit(B+"WC2026", WC+"note","LIVE snapshot — results as recorded by OpenFootball, community open data, not independently verified; semis + final still to be played.")
    for g,members in GROUPS.items():
        gi=B+"group/"+g; iri(gi, RDF, WC+"Group"); lit(gi, RDFS+"label", f"Group {g}"); iri(gi, DCT+"isPartOf", B+"WC2026")
        for t in members:
            ti=team_iri(t); iri(ti, RDF, SC+"SportsTeam"); lit(ti, SC+"name", t); iri(ti, WC+"inGroup", gi)

# ---- stadiums ----
def parse_coords(s):
    s=s.strip()
    m=re.match(r"""(\d+)°(\d+)'([\d.]+)"([NS])\s+(\d+)°(\d+)'([\d.]+)"([EW])""", s)
    if m:
        la=int(m[1])+int(m[2])/60+float(m[3])/3600; la=-la if m[4]=="S" else la
        lo=int(m[5])+int(m[6])/60+float(m[7])/3600; lo=-lo if m[8]=="W" else lo
        return la,lo
    m=re.match(r"([\d.]+)°([NS])\s+([\d.]+)°([EW])", s)
    if m:
        la=float(m[1]); la=-la if m[2]=="S" else la
        lo=float(m[3]); lo=-lo if m[4]=="W" else lo
        return la,lo
    return None
CITY2STAD={}
def emit_stadiums():
    for line in (RAW/"cup_stadiums.csv").read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or line.startswith("city") or not line.strip(): continue
        p=[x.strip() for x in line.split(",")]
        if len(p)<8: continue
        # city may itself contain a comma -> the last 7 are fixed columns
        coords=p[-1]; wd=p[-2]; wiki=p[-3]; cap=p[-4]; name=p[-5]; cc=p[-6]; tz=p[-7]; city=",".join(p[:-7]).strip()
        si=B+"stadium/"+slug(name); CITY2STAD[city]=si
        iri(si, RDF, WC+"Stadium"); lit(si, SC+"name", name); lit(si, WC+"city", city); lit(si, WC+"country", cc)
        if cap.isdigit(): lit(si, WC+"capacity", cap, dt=XSD+"integer")
        c=parse_coords(coords)
        if c: lit(si, GEO+"asWKT", f"POINT({c[1]:.5f} {c[0]:.5f})", dt=GEO+"wktLiteral")

# ---- matches ----
MATCH_N=[0]
def emit_matches(path, default_stage):
    lines=(RAW/path).read_text(encoding="utf-8").splitlines()
    cur_date=None; cur_stage=default_stage; cur_group=None; pending=None; sbuf=[""]
    date_re=re.compile(r"^(Mon|Tue|Wed|Thu|Fri|Sat|Sun)\s+(June|July|Jun|Jul|May)\s+(\d+)")
    def flush():
        if pending and "(" in sbuf[0]: emit_goals(pending, sbuf[0])
        sbuf[0]=""
    for raw in lines:
        s=raw.strip()
        if not s: continue
        mg=re.match(r"^▪?\s*Group ([A-L])\b", s)
        if mg: flush(); cur_group=mg[1]; cur_stage="group"; continue
        if s.startswith("▪"):
            flush()
            for st,key in [("Round of 32","round_of_32"),("Round of 16","round_of_16"),
                           ("Quarter-final","quarter_final"),("Semi-final","semi_final"),
                           ("third place","third_place"),("Final","final")]:
                if st.lower() in s.lower(): cur_stage=key; cur_group=None; break
            continue
        md=date_re.match(s)
        if md: flush(); cur_date=f"2026-{MONTHS[md[2]]:02d}-{int(md[3]):02d}"; continue
        if "@" not in s:
            if pending: sbuf[0]+=" "+s
            continue
        pre=s.split("@",1)[0]; venue=s.split("@",1)[1].strip()
        scored=re.search(r"(\d+)-(\d+)", pre)
        isfix=(" v " in pre or re.search(r"\bv\b", pre)) and not scored
        if not scored and not isfix:
            if pending: sbuf[0]+=" "+s
            continue
        ta,tb=teams_in(pre)
        if not ta or not tb:
            if pending: sbuf[0]+=" "+s
            continue
        flush()
        MATCH_N[0]+=1; mi=B+f"match/{MATCH_N[0]:03d}"
        iri(mi, RDF, WC+"Match"); iri(mi, DCT+"isPartOf", B+"WC2026"); lit(mi, WC+"stage", cur_stage)
        if cur_group: iri(mi, WC+"inGroup", B+"group/"+cur_group)
        if cur_date: lit(mi, SC+"startDate", cur_date, dt=XSD+"date")
        iri(mi, WC+"homeTeam", team_iri(ta)); iri(mi, WC+"awayTeam", team_iri(tb))
        city=venue.split("(")[0].strip()
        stad=CITY2STAD.get(city) or CITY2STAD.get(venue.strip())
        if stad: iri(mi, WC+"venue", stad)
        lit(mi, WC+"venueCity", city)
        if scored:
            hs,as_=int(scored[1]),int(scored[2]); lit(mi, RDFS+"label", f"{ta} {hs}-{as_} {tb}")
            lit(mi, WC+"homeScore", hs, dt=XSD+"integer"); lit(mi, WC+"awayScore", as_, dt=XSD+"integer")
            lit(mi, WC+"status","played")
            pen=re.search(r"(\d+)-(\d+)\s*pen", pre); aet="a.e.t" in pre.lower()
            if aet: lit(mi, WC+"afterExtraTime","true",dt=XSD+"boolean")
            if pen:
                lit(mi, WC+"homePenalties", pen[1], dt=XSD+"integer"); lit(mi, WC+"awayPenalties", pen[2], dt=XSD+"integer")
                winner=ta if int(pen[1])>int(pen[2]) else tb
            else: winner=ta if hs>as_ else (tb if as_>hs else None)
            if winner: iri(mi, WC+"winner", team_iri(winner))
            pending=(mi,ta,tb); sbuf[0]=""
        else:
            lit(mi, RDFS+"label", f"{ta} v {tb} (scheduled)"); lit(mi, WC+"status","scheduled")
            pending=None; sbuf[0]=""
    flush()

def emit_goals(pending, s):
    mi,ta,tb=pending
    body=s.strip().lstrip("(").rstrip(")").strip()
    halves=body.split(";",1)
    for half,team,opp in [(halves[0],ta,tb)]+([(halves[1],tb,ta)] if len(halves)>1 else []):
        for m in re.finditer(r"([A-ZÀ-Ýa-zà-ÿ' .\-]+?)\s+((?:\d+\+?\d*'?(?:\s*\([^)]*\))?[,\s]*)+)", half):
            name=m[1].strip(" .,")
            if len(name)<2 or name.lower() in ("pen","o.g","og","p"): continue
            mins=re.findall(r"(\d+\+?\d*)'", m[2]); og="o.g" in m[2].lower() or "(og" in m[2].lower()
            pen="(p" in m[2].lower() or "pen" in m[2].lower()
            scorer_team=opp if og else team; pi=player_iri(name)
            iri(pi, RDF, SC+"Person"); lit(pi, SC+"name", CANON.get(name,name))
            for mn in mins or [""]:
                gi=B+f"goal/{MATCH_N[0]:03d}_{slug(name)}_{mn or 'x'}"
                iri(gi, RDF, WC+"Goal"); iri(gi, WC+"inMatch", mi); iri(gi, WC+"scoredBy", pi)
                iri(gi, WC+"forTeam", team_iri(scorer_team))
                if mn: lit(gi, WC+"minute", re.sub(r"\+.*","",mn), dt=XSD+"integer")
                if og: lit(gi, WC+"ownGoal","true",dt=XSD+"boolean")
                if pen: lit(gi, WC+"penalty","true",dt=XSD+"boolean")
                iri(pi, WC+"scored", gi)

def main():
    parse_groups(); scan_names()
    emit_static(); emit_stadiums()
    emit_matches("cup.txt","group"); emit_matches("cup_finals.txt","knockout")
    OUT.write_text("\n".join(_out)+"\n", encoding="utf-8")
    print(f"triples: {len(_out):,}  teams: {len(set(TEAMS))}  matches: {MATCH_N[0]}  scorers: {len(PLAYERS)}")

if __name__=="__main__": main()
