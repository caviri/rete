#!/usr/bin/env python3
"""StatsBomb open-data (events + 360) for the 2022 World Cup FINAL -> wcfinal.json
for the freeze-frame replay.

StatsBomb "360" gives a freeze-frame at each event moment: every visible player's
(x,y) on a 120x80 pitch, flagged teammate/actor/keeper relative to the player on the
ball. There is no persistent player identity across frames and no separate ball track
(the ball ~ the event location) — so this is a sequence of REAL snapshots of the final,
stepped on the match clock, not continuous tracking. Each frame keeps the on-ball
player's name + event type so the caption reads e.g. '108:15 · Shot · Lionel Messi'.

Data: StatsBomb Open Data (github.com/statsbomb/open-data), free with attribution."""
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
D    = REPO / "data" / "wcfinal"
OUT  = D / "wcfinal.json"
HOME, AWAY = "Argentina", "France"

# StatsBomb stores full legal names; map the marquee players to their known short name
SHORT = {
    "Lionel Andrés Messi Cuccittini": "Messi", "Ángel Fabián Di María Hernández": "Di María",
    "Kylian Mbappé Lottin": "Mbappé", "Randal Kolo Muani": "Kolo Muani",
    "Gonzalo Ariel Montiel": "Montiel", "Paulo Bruno Exequiel Dybala": "Dybala",
    "Leandro Daniel Paredes": "Paredes", "Emiliano Martínez Romero": "E. Martínez",
    "Julián Álvarez": "J. Álvarez", "Alexis Mac Allister": "Mac Allister",
    "Rodrigo Javier De Paul": "De Paul", "Nicolás Alejandro Tagliafico": "Tagliafico",
    "Nahuel Molina Lucero": "Molina", "Cristian Gabriel Romero": "C. Romero",
    "Nicolás Hernán Gonzalo Otamendi": "Otamendi", "Antoine Griezmann": "Griezmann",
    "Ousmane Dembélé": "Dembélé", "Olivier Giroud": "Giroud",
    "Aurélien Djani Tchouaméni": "Tchouaméni", "Adrien Rabiot": "Rabiot",
    "Theo Bernard François Hernández": "T. Hernández", "Jules Koundé": "Koundé",
    "Dayot Upamecano": "Upamecano", "Raphaël Varane": "Varane", "Hugo Lloris": "Lloris",
    "Marcus Thuram": "Thuram", "Kingsley Coman": "Coman", "Eduardo Camavinga": "Camavinga",
}
def short(n):
    return SHORT.get(n) or (n.split()[-1] if n else "")

def main():
    events = json.load(open(D / "events.json", encoding="utf-8"))
    ts     = json.load(open(D / "threesixty.json", encoding="utf-8"))
    by_id  = {e["id"]: e for e in events}
    order  = {e["id"]: i for i, e in enumerate(events)}   # chronological order

    frames = []
    for f in ts:
        e = by_id.get(f.get("event_uuid"))
        if not e or not e.get("location"):
            continue
        team = e["team"]["name"]                          # the on-ball player's team
        other = AWAY if team == HOME else HOME
        pl = []
        for p in f["freeze_frame"]:
            loc = p.get("location")
            if not loc:
                continue
            side = "A" if (team if p["teammate"] else other) == HOME else "F"
            pl.append([round(loc[0], 1), round(loc[1], 1), side,
                       (1 if p["actor"] else 0) | (2 if p["keeper"] else 0)])
        frames.append({
            "t": round(e["minute"] * 60 + e["second"], 1),
            "p": e["period"],
            "ty": e["type"]["name"],
            "by": short((e.get("player") or {}).get("name", "")),
            "tm": "A" if team == HOME else "F",
            "b": [round(e["location"][0], 1), round(e["location"][1], 1)],
            "pl": pl,
            "_o": order[e["id"]],
        })
    frames.sort(key=lambda fr: fr["_o"])
    for fr in frames:
        del fr["_o"]

    # goals (open play + penalties in normal time) for the jump list
    goals = []
    for e in events:
        nm = e["type"]["name"]
        if nm == "Shot" and (e.get("shot", {}).get("outcome", {}) or {}).get("name") == "Goal":
            goals.append({"t": round(e["minute"] * 60 + e["second"], 1), "min": e["minute"],
                          "player": short(e["player"]["name"]), "team": "A" if e["team"]["name"] == HOME else "F", "period": e["period"],
                          "pen": e.get("shot", {}).get("type", {}).get("name") == "Penalty"})
        elif nm == "Own Goal Against":
            goals.append({"t": round(e["minute"] * 60 + e["second"], 1), "min": e["minute"],
                          "player": short(e["player"]["name"]) + " (OG)",
                          "team": "A" if e["team"]["name"] == HOME else "F", "period": e["period"], "pen": False})
    goals.sort(key=lambda g: g["t"])

    doc = {
        "match": {"home": HOME, "away": AWAY,
                  "competition": "FIFA World Cup 2022 — Final",
                  "venue": "Lusail Stadium", "date": "2022-12-18",
                  "result": "3–3 · Argentina win 4–2 on penalties",
                  "source": "StatsBomb Open Data",
                  "license": "StatsBomb Public Data — free for public use with attribution",
                  "pitch": [120, 80]},
        "colors": {"A": "#6ca6e8", "F": "#e0554e"},
        "goals": goals,
        "frames": frames,
    }
    OUT.write_text(json.dumps(doc, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    print(f"frames: {len(frames)}, goals: {len(goals)}, json: {OUT.stat().st_size/1e6:.2f} MB -> {OUT}")
    print("goals:", ", ".join(f"{g['min']}' {g['player']}({g['team']}){'(pen)' if g['pen'] else ''}" for g in goals))

if __name__ == "__main__":
    main()
