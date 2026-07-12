#!/usr/bin/env python3
"""tears_of_steel.nt -> compact subtitles.json for the karaoke timeline viewer.

Each su:Line is a moment (start/end seconds) carrying the same utterance in every
language; the viewer shows the active line's text in all languages at once as you
scrub. Tiny payload (~80 KB)."""
import json, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
NT   = REPO / "data" / "subtitles" / "tears_of_steel.nt"
OUT  = REPO / "data" / "subtitles" / "subtitles.json"
SU   = "https://w3id.org/rete/subtitles#"

def unescape(s):
    return (s.replace('\\"', '"').replace("\\n", " ").replace("\\t", " ")
             .replace("\\\\", "\\"))

# object literal with optional @lang:  "..."@lang  or  "..."^^<type>  or  "..."
LIT = re.compile(r'^"(.*)"(?:@([\w-]+)|\^\^<[^>]+>)?\s*$')

def obj_value(o):
    m = LIT.match(o.strip())
    if not m:
        return None, None
    return unescape(m.group(1)), m.group(2)

def main():
    lines = {}            # ms(int) -> {"i":idx,"start":s,"end":e,"text":{lang:txt}}
    tracks = {}           # lang -> {"name":..,"dir":..}
    for raw in NT.open(encoding="utf-8"):
        m = re.match(r'<[^>]*/line/(\d+)> <([^>]+)> (.+) \.$', raw)
        if m:
            key = int(m.group(1)); p = m.group(2); val, lang = obj_value(m.group(3))
            L = lines.setdefault(key, {"i": None, "start": None, "end": None, "text": {}})
            if p == SU + "start":   L["start"] = float(val)
            elif p == SU + "end":   L["end"] = float(val)
            elif p == SU + "index": L["i"] = int(val)
            elif p == SU + "text" and lang: L["text"][lang] = val
            continue
        m = re.match(r'<[^>]*/track/([\w-]+)> <([^>]+)> (.+) \.$', raw)
        if m:
            lang = m.group(1); p = m.group(2); val, _ = obj_value(m.group(3))
            tk = tracks.setdefault(lang, {"name": lang, "dir": "ltr"})
            if p.endswith("#label"):      tk["name"] = val
            elif p == SU + "direction":   tk["dir"] = val

    out_lines = [lines[k] for k in sorted(lines)]
    # languages present, English first then alphabetical by name
    langs = sorted(tracks.keys(), key=lambda c: (c != "en", tracks[c]["name"]))
    out_langs = [{"code": c, "name": tracks[c]["name"], "dir": tracks[c]["dir"]} for c in langs]

    doc = {
        "work": {"title": "Tears of Steel", "license": "CC BY 3.0",
                 "url": "https://mango.blender.org/",
                 "source": "https://media.xiph.org/mango/subtitles/"},
        "languages": out_langs,
        "lines": out_lines,
    }
    OUT.write_text(json.dumps(doc, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    print(f"languages: {len(out_langs)}, lines: {len(out_lines)}, "
          f"json: {OUT.stat().st_size/1024:.1f} KB -> {OUT}")

if __name__ == "__main__":
    main()
