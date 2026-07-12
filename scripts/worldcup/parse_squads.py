#!/usr/bin/env python3
"""Parse full World Cup squads from the Wikipedia '<year> FIFA World Cup squads'
wikitext and append them to the graph — the FULL rosters (not just scorers).

Each player is one line: {{nat fs g player|no=|pos=|name=[[..]]|age={{birth date
and age2|..}}|caps=|goals=|club=[[..]]}}. Emits a Person with shirt number,
position, DOB, caps, and club (at the tournament) + their national team. Player
IRIs reuse the name slug so squad players merge with the scorer nodes already in
the graph.

Usage: python scripts/worldcup/parse_squads.py <wiki_file> <nt_file> <base_iri>
"""
import re, sys, urllib.parse
from pathlib import Path

WIKI, NT, B = sys.argv[1], sys.argv[2], sys.argv[3]
WC="https://w3id.org/rete/worldcup#"; SC="http://schema.org/"; RDF="http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS="http://www.w3.org/2000/01/rdf-schema#"; XSD="http://www.w3.org/2001/XMLSchema#"
def slug(s): return urllib.parse.quote(s.strip().replace(" ","_"), safe="")
def esc(s): return str(s).replace("\\","\\\\").replace('"','\\"').replace("\n"," ").replace("\r"," ").replace("\t"," ")

ALIAS={"United States":"USA","IR Iran":"Iran","Korea Republic":"South Korea",
       "Bosnia and Herzegovina":"Bosnia & Herzegovina","Türkiye":"Turkey"}

def linkname(v):
    m=re.search(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]", v)
    if not m: return None
    n=(m.group(2) or m.group(1)).strip()
    return re.sub(r"\s*\(.*?\)\s*$","",n).strip()

def main():
    text=Path(WIKI).read_text(encoding="utf-8")
    out=[]; teams=0; players=0
    parts=re.split(r"\n===\s*([^=\n]+?)\s*===\n", text)
    for i in range(1, len(parts)-1, 2):
        team=ALIAS.get(parts[i].strip(), parts[i].strip()); ti=B+"team/"+slug(team)
        rows=re.findall(r"\{\{nat fs g?\s*player\s*(\|.+)", parts[i+1])   # one player per line (keep leading |)
        if not rows: continue
        teams+=1
        for r in rows:
            name=linkname((re.search(r"\|?name=(\[\[[^\]]+(?:\|[^\]]+)?\]\])", r) or re.search(r"name=(\[\[[^\]]+\]\])", r) or type("",(),{"group":lambda s,x:""})()).group(1) if re.search(r"name=\[\[", r) else "")
            if not name: continue
            pi=B+"player/"+slug(name)
            out.append(f'<{pi}> <{RDF}> <{SC}Person> .')
            out.append(f'<{pi}> <{SC}name> "{esc(name)}" .')
            out.append(f'<{pi}> <{WC}representedTeam> <{ti}> .')
            out.append(f'<{ti}> <{WC}squadPlayer> <{pi}> .')
            pos=re.search(r"\|pos=([A-Za-z]+)", r);  no=re.search(r"\|no=(\d+)", r); caps=re.search(r"\|caps=(\d+)", r)
            if pos: out.append(f'<{pi}> <{WC}position> "{pos.group(1)}" .')
            if no: out.append(f'<{pi}> <{WC}shirtNumber> "{no.group(1)}"^^<{XSD}integer> .')
            if caps: out.append(f'<{pi}> <{WC}caps> "{caps.group(1)}"^^<{XSD}integer> .')
            am=re.search(r"\{\{[Bb]irth date and age2?[^}]*\}\}", r)
            if am:
                nums=re.findall(r"\d+", am.group(0))
                if len(nums)>=3:
                    y,mo,d=nums[-3:]
                    if len(y)==4: out.append(f'<{pi}> <{SC}birthDate> "{int(y):04d}-{int(mo):02d}-{int(d):02d}"^^<{XSD}date> .')
            club=linkname((re.search(r"\|club=(\[\[[^\]]+(?:\|[^\]]+)?\]\])", r) or type("",(),{"group":lambda s,x:""})()).group(1) if re.search(r"\|club=\[\[", r) else "")
            if club:
                ci=B+"club/"+slug(club)
                out.append(f'<{pi}> <{WC}clubAtTournament> <{ci}> .')
                out.append(f'<{ci}> <{RDFS}label> "{esc(club)}" .')
                out.append(f'<{ci}> <{RDF}> <{SC}SportsTeam> .')
            players+=1
    with open(NT,"a",encoding="utf-8") as fh: fh.write("\n".join(out)+"\n")
    print(f"squads: {teams} teams, {players} players -> appended to {NT}")

if __name__=="__main__": main()
