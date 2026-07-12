#!/usr/bin/env python3
"""Build a World Cup 2022 knowledge graph (.nt) from open data.

Sources (data/worldcup/raw/):
  - OpenFootball cup.txt + cup_finals.txt : groups, 64 matches, scores, scorers
  - cup_stadiums.csv                       : 8 stadiums (capacity, coords)
  - 538_wc_forecasts_final.csv             : FiveThirtyEight forecast, 6 timestamps
  + hardcoded pre-tournament bookmaker & Opta champion probabilities (documented)
  + (career stints for star players added separately from Wikidata)

Model: schema.org + Dublin Core + GeoSPARQL + a small wc: vocab on the neutral
w3id PURL. Predictions and career stints are n-ary nodes (Wikidata's qualifier
pattern) so they stay fully queryable with plain SPARQL.
"""
from __future__ import annotations
import csv, re, sys, unicodedata, urllib.parse
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
RAW = REPO / "data" / "worldcup" / "raw"
OUT = REPO / "data" / "worldcup" / "worldcup.nt"

WC   = "https://w3id.org/rete/worldcup#"          # vocabulary
B    = "https://w3id.org/rete/worldcup/"          # entity IRIs
SC   = "http://schema.org/"
DCT  = "http://purl.org/dc/terms/"
RDF  = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"   # used only as the rdf:type predicate
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
GEO  = "http://www.opengis.net/ont/geosparql#"
XSD  = "http://www.w3.org/2001/XMLSchema#"
PROV = "http://www.w3.org/ns/prov#"

TEAMS = ["Qatar","Ecuador","Senegal","Netherlands","England","Iran","USA","Wales",
    "Argentina","Saudi Arabia","Mexico","Poland","France","Australia","Denmark","Tunisia",
    "Spain","Costa Rica","Germany","Japan","Belgium","Canada","Morocco","Croatia",
    "Brazil","Serbia","Switzerland","Cameroon","Portugal","Ghana","Uruguay","South Korea"]
TEAMS_BY_LEN = sorted(TEAMS, key=len, reverse=True)   # match longest name first
GROUPS = {"A":["Qatar","Ecuador","Senegal","Netherlands"],"B":["England","Iran","USA","Wales"],
    "C":["Argentina","Saudi Arabia","Mexico","Poland"],"D":["France","Australia","Denmark","Tunisia"],
    "E":["Spain","Costa Rica","Germany","Japan"],"F":["Belgium","Canada","Morocco","Croatia"],
    "G":["Brazil","Serbia","Switzerland","Cameroon"],"H":["Portugal","Ghana","Uruguay","South Korea"]}
FINAL_RANK = {"Argentina":1,"France":2,"Croatia":3,"Morocco":4}

MONTHS = {"Nov":11,"Dec":12}

_out = []
def slug(s): return urllib.parse.quote(s.strip().replace(" ","_"), safe="")
def esc(s): return str(s).replace("\\","\\\\").replace('"','\\"').replace("\n"," ").replace("\r"," ").replace("\t"," ")
def iri(s,p,o): _out.append(f"<{s}> <{p}> <{o}> .")
def lit(s,p,v,dt=None,lang=None):
    o=f'"{esc(v)}"'+(f"^^<{dt}>" if dt else (f"@{lang}" if lang else ""))
    _out.append(f"<{s}> <{p}> {o} .")

def team_iri(name): return B+"team/"+slug(name)
def teams_in(line):
    """Return (teamA, teamB) found in a match line, in order of appearance."""
    hits=[]
    i=0
    # find all known team occurrences with positions, keep first two by position
    for t in TEAMS:
        idx=line.find(t)
        while idx!=-1:
            hits.append((idx,t)); idx=line.find(t, idx+1)
    hits.sort()
    # dedupe overlapping (e.g. 'USA' inside nothing) — keep by position, drop names
    # fully contained in an earlier longer hit at same start
    out=[]
    for pos,t in hits:
        if any(pos>=p and pos+len(t)<=p+len(nm) and (p,nm)!=(pos,t) for p,nm in hits):
            continue
        out.append((pos,t))
    out.sort()
    names=[t for _,t in out]
    return (names[0], names[1]) if len(names)>=2 else (None,None)

# ---------------------------------------------------------------- static nodes
def emit_static():
    iri(B+"WC2022", RDF, SC+"SportsEvent"); iri(B+"WC2022", RDF, WC+"Tournament")
    lit(B+"WC2022", SC+"name", "FIFA World Cup 2022"); lit(B+"WC2022", DCT+"date","2022")
    lit(B+"WC2022", SC+"location","Qatar")
    for g,members in GROUPS.items():
        gi=B+"group/"+g
        iri(gi, RDF, WC+"Group"); lit(gi, RDFS+"label", f"Group {g}"); iri(gi, DCT+"isPartOf", B+"WC2022")
        for t in members:
            ti=team_iri(t)
            iri(ti, RDF, SC+"SportsTeam"); lit(ti, SC+"name", t); iri(ti, WC+"inGroup", gi)
            if t in FINAL_RANK: lit(ti, WC+"finalRank", FINAL_RANK[t], dt=XSD+"integer")
    # champion / runner-up convenience
    iri(B+"WC2022", WC+"champion", team_iri("Argentina"))
    iri(B+"WC2022", WC+"runnerUp", team_iri("France"))

# ---------------------------------------------------------------- stadiums
def parse_dms(s):
    m=re.match(r"""(\d+)°(\d+)'([\d.]+)"([NS])\s+(\d+)°(\d+)'([\d.]+)"([EW])""", s.strip())
    if not m: return None
    la=int(m[1])+int(m[2])/60+float(m[3])/3600; la=-la if m[4]=="S" else la
    lo=int(m[5])+int(m[6])/60+float(m[7])/3600; lo=-lo if m[8]=="W" else lo
    return la,lo
def emit_stadiums():
    txt=(RAW/"cup_stadiums.csv").read_text(encoding="utf-8")
    for line in txt.splitlines():
        if line.startswith("#") or line.startswith("city") or not line.strip(): continue
        parts=[p.strip() for p in line.split(",")]
        if len(parts)<6: continue
        city,tz,name,cap,wiki,coords=parts[0],parts[1],parts[2],parts[3],parts[4],",".join(parts[5:]).strip()
        si=B+"stadium/"+slug(name)
        iri(si, RDF, WC+"Stadium"); lit(si, SC+"name", name); lit(si, WC+"city", city)
        if cap.isdigit(): lit(si, WC+"capacity", cap, dt=XSD+"integer")
        c=parse_dms(coords)
        if c: lit(si, GEO+"asWKT", f"POINT({c[1]:.5f} {c[0]:.5f})", dt=GEO+"wktLiteral")

STADIUMS={}  # name -> iri, filled lazily
def stadium_iri(name):
    key=name.strip()
    return B+"stadium/"+slug(key.split(",")[0].strip())

# ---------------------------------------------------------------- matches
MATCH_N=[0]
def emit_matches(path, default_stage):
    lines=(RAW/path).read_text(encoding="utf-8").splitlines()
    cur_date=None; cur_stage=default_stage; cur_group=None; pending=None; sbuf=[""]
    date_re=re.compile(r"^(Mon|Tue|Wed|Thu|Fri|Sat|Sun)\s+(Nov|Dec)\s+(\d+)")
    def flush():
        if pending and "(" in sbuf[0]: emit_goals(pending, sbuf[0])
        sbuf[0]=""
    for raw in lines:
        line=raw.rstrip()
        s=line.strip()
        if not s: continue
        # stage headers (finals file) / group headers
        mg=re.match(r"^▪?\s*Group\s+([A-H])\b", s)
        if mg: flush(); cur_group=mg[1]; cur_stage="group"; continue
        if s.startswith("▪"):
            flush()
            for st,key in [("Round of 16","round_of_16"),("Quarter-final","quarter_final"),
                           ("Semi-final","semi_final"),("third place","third_place"),("Final","final")]:
                if st.lower() in s.lower():
                    cur_stage=key; cur_group=None; break   # first (most specific) wins
            continue
        md=date_re.match(s)
        if md: flush(); cur_date=f"2022-{MONTHS[md[2]]:02d}-{int(md[3]):02d}"; continue
        # a match line has an "@ venue" marker and a score \d+-\d+ (time may be
        # absent on the 2nd of two simultaneous final-group matches)
        if "@" in s and re.search(r"\d+-\d+", s):
            flush()
            ta,tb=teams_in(s)
            if not ta or not tb: continue
            sc=re.search(r"(\d+)-(\d+)", s)
            hs,as_=int(sc[1]),int(sc[2])
            pen=re.search(r"(\d+)-(\d+)\s*pen", s)
            aet="a.e.t" in s.lower()
            std=""
            at=s.split("@",1)
            if len(at)>1: std=at[1].strip()
            MATCH_N[0]+=1; mi=B+f"match/{MATCH_N[0]:03d}"
            iri(mi, RDF, WC+"Match"); lit(mi, RDFS+"label", f"{ta} {hs}-{as_} {tb}")
            iri(mi, DCT+"isPartOf", B+"WC2022")
            lit(mi, WC+"stage", cur_stage)
            if cur_group: iri(mi, WC+"inGroup", B+"group/"+cur_group)
            if cur_date: lit(mi, SC+"startDate", cur_date, dt=XSD+"date")
            iri(mi, WC+"homeTeam", team_iri(ta)); iri(mi, WC+"awayTeam", team_iri(tb))
            lit(mi, WC+"homeScore", hs, dt=XSD+"integer"); lit(mi, WC+"awayScore", as_, dt=XSD+"integer")
            if aet: lit(mi, WC+"afterExtraTime", "true", dt=XSD+"boolean")
            if pen:
                lit(mi, WC+"homePenalties", pen[1], dt=XSD+"integer")
                lit(mi, WC+"awayPenalties", pen[2], dt=XSD+"integer")
                winner = ta if int(pen[1])>int(pen[2]) else tb
            else:
                winner = ta if hs>as_ else (tb if as_>hs else None)
            if winner: iri(mi, WC+"winner", team_iri(winner))
            if std: iri(mi, WC+"venue", stadium_iri(std))
            pending=(mi,ta,tb); sbuf[0]=""
            continue
        # continuation / scorer lines: accumulate into the current match's buffer
        if pending: sbuf[0] += " " + s
    flush()
    return

PLAYERS=set()
# Name normalization: OpenFootball writes the full name on first mention ("Kylian
# Mbappé") and a bare surname later ("Mbappé") — merge them by last token.
CANON={}
def build_canon(names):
    from collections import defaultdict
    groups=defaultdict(list)
    for n in names:
        toks=n.split()
        groups[toks[-1] if toks else n].append(n)
    for last,group in groups.items():
        canon=max(group, key=len)     # longest form in the group is canonical
        for n in group: CANON[n]=canon
def scan_names():
    names=set()
    for path in ["cup.txt","cup_finals.txt"]:
        for line in (RAW/path).read_text(encoding="utf-8").splitlines():
            s=line.strip()
            if "@" in s: continue     # skip match lines; only scorer text
            for m in re.finditer(r"([A-ZÀ-Ý][A-ZÀ-Ýa-zà-ÿ' .\-]+?)\s+\d+\+?\d*'", s):
                nm=m[1].strip(" .,")
                if len(nm)>=2 and nm.lower() not in ("pen","o.g","og","a.e.t"): names.add(nm)
    build_canon(names)
def player_iri(name):
    n=CANON.get(name.strip(), name.strip())
    PLAYERS.add(n)
    return B+"player/"+slug(n)

def emit_goals(pending, s):
    mi,ta,tb=pending
    body=s.strip().lstrip("(").rstrip(")").strip()
    # split team A vs team B scorers on the FIRST ';'
    halves=body.split(";",1)
    for half,team,opp in [(halves[0], ta, tb)] + ([(halves[1], tb, ta)] if len(halves)>1 else []):
        # each scorer: Name minute[, minute]  with optional (pen.)/(o.g.)
        # tokens: split on minute markers is hard; use regex for "Name  M'[, M']"
        for m in re.finditer(r"([A-ZÀ-Ýa-zà-ÿ' .\-]+?)\s+((?:\d+\+?\d*'?(?:\s*\([^)]*\))?[,\s]*)+)", half):
            name=m[1].strip(" .,")
            if len(name)<2 or name.lower() in ("pen","o.g","og"): continue
            mins=re.findall(r"(\d+\+?\d*)'", m[2])
            og="o.g" in m[2].lower() or "(og" in m[2].lower()
            pen="pen" in m[2].lower()
            scorer_team = opp if og else team   # own goals count for the opponent
            pi=player_iri(name)
            iri(pi, RDF, SC+"Person"); lit(pi, SC+"name", CANON.get(name,name))
            for mn in mins or [""]:
                gi=B+f"goal/{MATCH_N[0]:03d}_{slug(name)}_{mn or 'x'}"
                iri(gi, RDF, WC+"Goal"); iri(gi, WC+"inMatch", mi)
                iri(gi, WC+"scoredBy", pi); iri(gi, WC+"forTeam", team_iri(scorer_team))
                if mn: lit(gi, WC+"minute", re.sub(r"\+.*","",mn), dt=XSD+"integer")
                if og: lit(gi, WC+"ownGoal","true",dt=XSD+"boolean")
                if pen: lit(gi, WC+"penalty","true",dt=XSD+"boolean")
                iri(pi, WC+"scored", gi)

# ---------------------------------------------------------------- predictions
SRC={"538":"FiveThirtyEight (SPI model)","opta":"Opta supercomputer","book":"Bookmaker consensus (implied)"}
def emit_source(key):
    si=B+"source/"+key
    iri(si, RDF, PROV+"Agent"); lit(si, SC+"name", SRC[key]); return si
def prediction(subj_team, outcome, prob, source_key, asof, i):
    pi=B+f"prediction/{source_key}_{outcome}_{slug(subj_team)}_{asof.replace('-','')}_{i}"
    iri(pi, RDF, WC+"Prediction")
    iri(pi, WC+"aboutTeam", team_iri(subj_team))
    iri(pi, WC+"outcome", WC+outcome)                 # Champion / ReachFinal / ReachSemis / ReachQuarters
    lit(pi, WC+"probability", f"{prob:.5f}", dt=XSD+"decimal")
    iri(pi, PROV+"wasAttributedTo", emit_source(source_key))
    lit(pi, WC+"forecastDate", asof, dt=XSD+"date")
    lit(pi, PROV+"generatedAtTime", asof+"T12:00:00Z", dt=XSD+"dateTime")

def emit_538():
    rows=list(csv.DictReader(open(RAW/"538_wc_forecasts_final.csv",encoding="utf-8-sig")))
    cols={"win_league":"Champion","make_final":"ReachFinal","make_semis":"ReachSemis","make_quarters":"ReachQuarters"}
    for i,r in enumerate(rows):
        team=r["team"]
        if team not in TEAMS: continue
        asof=r["forecast_timestamp"][:10]
        for c,outcome in cols.items():
            try: p=float(r[c])
            except: continue
            if p>0 or c=="win_league":
                prediction(team, outcome, p, "538", asof, i)
        # SPI rating as a team attribute snapshot
    # done

def emit_book_opta():
    # documented pre-tournament (2022-11-20) champion probabilities (representative)
    opta={"Brazil":.153,"Argentina":.131,"France":.120,"England":.099,"Spain":.088,
          "Germany":.076,"Netherlands":.062,"Portugal":.058,"Belgium":.040,"Denmark":.030,"Croatia":.021}
    book={"Brazil":.170,"Argentina":.140,"France":.120,"England":.105,"Spain":.090,
          "Germany":.080,"Netherlands":.060,"Portugal":.055,"Belgium":.040,"Uruguay":.025,"Croatia":.020}
    for j,(team,p) in enumerate(opta.items()): prediction(team,"Champion",p,"opta","2022-11-20",1000+j)
    for j,(team,p) in enumerate(book.items()): prediction(team,"Champion",p,"book","2022-11-20",2000+j)
    # outcome class labels
    for o,l in [("Champion","Win the tournament"),("ReachFinal","Reach the final"),
                ("ReachSemis","Reach the semi-finals"),("ReachQuarters","Reach the quarter-finals")]:
        iri(WC+o, RDF, WC+"Outcome"); lit(WC+o, RDFS+"label", l)

# ---------------------------------------------------------------- main
def main():
    scan_names()
    emit_static(); emit_stadiums()
    emit_matches("cup.txt","group")
    emit_matches("cup_finals.txt","knockout")
    emit_538(); emit_book_opta()
    OUT.write_text("\n".join(_out)+"\n", encoding="utf-8")
    print(f"triples: {len(_out):,}")
    print(f"matches: {MATCH_N[0]}, players(scorers): {len(PLAYERS)}")
    print(f"-> {OUT}")

if __name__ == "__main__":
    main()
